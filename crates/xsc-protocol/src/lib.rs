//! Shared command and transport contract for the `xs` client and daemon.
//!
//! This crate intentionally does not depend on the terminal emulator or on
//! clap. Both process boundaries can therefore evolve independently while
//! sharing only serializable command data and the framed loopback transport.

use std::collections::HashMap;
use std::path::PathBuf;

use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

/// Wire protocol version understood by this build.
pub const PROTOCOL_VERSION: u32 = 2;

/// Maximum serialized frame size.
pub const MAX_FRAME: usize = 16 * 1024 * 1024;

/// Request understood by the daemon.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Request {
    /// Negotiate the wire protocol before sending another request.
    Hello {
        /// Protocol version implemented by the client.
        protocol_version: u32,
    },
    /// Probe daemon liveness.
    Ping,
    /// Read daemon state.
    Status,
    /// Stop the daemon.
    Stop,
    /// Execute one terminal-management command.
    Command {
        /// Serializable command payload.
        command: CommandRequest,
    },
}

/// Serializable command sent across the process boundary.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum CommandRequest {
    /// Pane and window management.
    Tml(TmlCommand),
    /// tmux session management.
    Tmx(TmxCommand),
    /// Print the layout tree.
    Layout {
        /// Optional window identifier.
        window: Option<String>,
    },
    /// List resources.
    Ls(LsCommand),
    /// Print command help.
    Help,
}

/// `tml` command payloads.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum TmlCommand {
    /// Create a local, SSH, WSL, or tmux pane/window.
    Add(TmlAddRequest),
    /// Delete a pane.
    Del {
        /// Target pane identifier.
        pane: String,
    },
    /// Send text to a pane.
    Write {
        /// Target pane identifier.
        pane: String,
        /// Text fragments joined by spaces.
        text: Vec<String>,
    },
    /// Read a rendered pane snapshot.
    Read {
        /// Target pane identifier.
        pane: String,
    },
    /// Resize a pane.
    Resize {
        /// Target pane identifier.
        pane: String,
        /// New column count.
        cols: u16,
        /// New row count.
        rows: u16,
    },
    /// Move focus to a pane.
    Focus {
        /// Target pane identifier.
        pane: String,
    },
    /// Fetch tmux scrollback.
    Fetch {
        /// Target pane identifier.
        pane: String,
        /// Optional number of recent lines.
        line: Option<usize>,
    },
    /// Start recording pane output.
    Log(TmlLogRequest),
}

/// Parameters for creating a pane or window.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TmlAddRequest {
    /// Conversation URL, omitted for tmux groups.
    pub url: Option<String>,
    /// Target group name or identifier.
    pub group: Option<String>,
    /// Request creation in a new window.
    pub new_window: bool,
    /// Optional horizontal split target.
    pub sph: Option<String>,
    /// Optional vertical split target.
    pub spv: Option<String>,
    /// Client-resolved password.
    pub password: Option<String>,
}

/// Output logger format.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LogFormat {
    /// Record raw terminal bytes.
    Raw,
    /// Record ANSI-filtered timestamped lines.
    Char,
}

/// Parameters for recording pane output.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TmlLogRequest {
    /// Target pane identifier.
    pub pane: String,
    /// Logger output format.
    pub format: LogFormat,
    /// Maximum bytes for raw output or lines for character output.
    pub max_size: usize,
    /// Optional output path.
    pub path: Option<PathBuf>,
}

/// `tmx` command payloads.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum TmxCommand {
    /// Attach to a tmux session.
    Attach(TmxAttachRequest),
    /// Reconcile one group with its tmux server.
    Sync {
        /// Group identifier or name.
        group_id: String,
    },
}

/// Parameters for attaching to tmux.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TmxAttachRequest {
    /// tmux URL.
    pub url: String,
    /// Optional SSH endpoint URL.
    pub ssh: Option<String>,
    /// Optional WSL endpoint URL.
    pub wsl: Option<String>,
}

/// `ls` command payloads.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum LsCommand {
    /// List panes.
    Pane,
    /// List windows, optionally scoped to a group.
    Window {
        /// Optional group identifier or name.
        group: Option<String>,
    },
    /// List groups.
    Group,
}

/// Generic response returned by the daemon.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Response {
    /// Whether the request succeeded.
    pub ok: bool,
    /// Error text when the request failed.
    #[serde(default)]
    pub error: Option<String>,
    /// Text output when the request succeeded.
    #[serde(default)]
    pub output: String,
}

impl Response {
    /// Construct a successful response.
    pub fn ok(output: String) -> Self {
        Self {
            ok: true,
            error: None,
            output,
        }
    }

    /// Construct a failed response.
    pub fn fail(error: impl Into<String>) -> Self {
        Self {
            ok: false,
            error: Some(error.into()),
            output: String::new(),
        }
    }
}

/// Conversation specification parsed from a `tml add` URL.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConversationSpec {
    /// Local shell process.
    Shell {
        /// Program to execute.
        program: String,
        /// Program arguments.
        args: Vec<String>,
        /// Optional working directory.
        cwd: Option<String>,
        /// Initial column count.
        cols: u16,
        /// Initial row count.
        rows: u16,
    },
    /// SSH shell process.
    Ssh {
        /// `user@host[:port]` target.
        target: String,
        /// Optional identity path.
        identity: Option<String>,
        /// Optional password.
        password: Option<String>,
        /// Initial column count.
        cols: u16,
        /// Initial row count.
        rows: u16,
    },
    /// WSL shell process.
    Wsl {
        /// Arguments passed to `wsl.exe`.
        args: Vec<String>,
        /// Initial column count.
        cols: u16,
        /// Initial row count.
        rows: u16,
    },
}

impl ConversationSpec {
    /// Return the conversation URL scheme.
    pub fn scheme(&self) -> &'static str {
        match self {
            Self::Shell { .. } => "shell",
            Self::Ssh { .. } => "ssh",
            Self::Wsl { .. } => "wsl",
        }
    }

    /// Return the initial column count.
    pub fn cols(&self) -> u16 {
        match self {
            Self::Shell { cols, .. } | Self::Ssh { cols, .. } | Self::Wsl { cols, .. } => *cols,
        }
    }

    /// Return the initial row count.
    pub fn rows(&self) -> u16 {
        match self {
            Self::Shell { rows, .. } | Self::Ssh { rows, .. } | Self::Wsl { rows, .. } => *rows,
        }
    }
}

/// Parse `scheme://path?key=value&...`.
pub fn parse_conversation_url(url: &str) -> Result<ConversationSpec> {
    let (scheme, rest) = url
        .split_once("://")
        .ok_or_else(|| anyhow!("expected scheme://... URL, got {url}"))?;
    let (path, query) = match rest.split_once('?') {
        Some((path, query)) => (path, query),
        None => (rest, ""),
    };
    let params = parse_query(query);

    match scheme {
        "shell" => {
            if path.is_empty() {
                return Err(anyhow!("shell URL requires a program: shell://<program>"));
            }
            let (cols, rows) = parse_size(&params)?;
            Ok(ConversationSpec::Shell {
                program: path.to_string(),
                args: parse_args_param(&params)?,
                cwd: params.get("cwd").cloned(),
                cols,
                rows,
            })
        }
        "ssh" => {
            if path.is_empty() {
                return Err(anyhow!("ssh URL requires user@host[:port]"));
            }
            let (cols, rows) = parse_size(&params)?;
            Ok(ConversationSpec::Ssh {
                target: path.to_string(),
                identity: params.get("identity").cloned(),
                password: params.get("password").cloned(),
                cols,
                rows,
            })
        }
        "wsl" => {
            let (cols, rows) = parse_size(&params)?;
            Ok(ConversationSpec::Wsl {
                args: parse_args_param(&params)?,
                cols,
                rows,
            })
        }
        other => Err(anyhow!("unsupported conversation scheme: {other}")),
    }
}

/// Parse `user@host[:port]`.
pub fn parse_ssh_target(target: &str) -> Result<(String, String, u16)> {
    let (user, rest) = target
        .split_once('@')
        .ok_or_else(|| anyhow!("expected user@host[:port], got {target}"))?;
    let (host, port) = match rest.split_once(':') {
        Some((host, port)) => (
            host.to_string(),
            port.parse::<u16>().map_err(|_| anyhow!("invalid port"))?,
        ),
        None => (rest.to_string(), 22),
    };
    if user.is_empty() || host.is_empty() {
        return Err(anyhow!("invalid ssh target {target}"));
    }
    Ok((user.to_string(), host, port))
}

fn parse_query(query: &str) -> HashMap<String, String> {
    let mut params = HashMap::new();
    for pair in query.split('&').filter(|pair| !pair.is_empty()) {
        if let Some((key, value)) = pair.split_once('=') {
            params.insert(key.to_string(), value.to_string());
        }
    }
    params
}

fn parse_size(params: &HashMap<String, String>) -> Result<(u16, u16)> {
    let cols = match params.get("cols") {
        Some(value) => value
            .parse::<u16>()
            .map_err(|_| anyhow!("invalid cols: {value}"))?,
        None => 80,
    };
    let rows = match params.get("rows") {
        Some(value) => value
            .parse::<u16>()
            .map_err(|_| anyhow!("invalid rows: {value}"))?,
        None => 24,
    };
    Ok((cols, rows))
}

fn parse_args_param(params: &HashMap<String, String>) -> Result<Vec<String>> {
    match params.get("args") {
        Some(value) if value.is_empty() => Ok(Vec::new()),
        Some(value) => Ok(value.split(',').map(str::to_string).collect()),
        None => Ok(Vec::new()),
    }
}

/// Derive the per-user loopback port used by the daemon.
pub fn port() -> u16 {
    #[cfg(windows)]
    let identity = std::env::var("USERPROFILE").unwrap_or_default();
    #[cfg(not(windows))]
    let identity = std::env::var("HOME").unwrap_or_default();
    let hash = fnv1a(identity.as_bytes());
    40_000 + (hash % 20_000) as u16
}

fn fnv1a(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

/// Connect to the current user's daemon.
pub async fn connect() -> std::io::Result<tokio::net::TcpStream> {
    let stream = tokio::net::TcpStream::connect(("127.0.0.1", port())).await?;
    stream.set_nodelay(true)?;
    Ok(stream)
}

/// Check whether the daemon port accepts TCP connections.
pub async fn probe() -> bool {
    connect().await.is_ok()
}

/// Write one length-prefixed JSON frame.
pub async fn write_frame<W>(writer: &mut W, value: &impl Serialize) -> Result<()>
where
    W: AsyncWrite + Unpin + ?Sized,
{
    let body = serde_json::to_vec(value).context("serialize frame")?;
    if body.len() > MAX_FRAME {
        anyhow::bail!("frame body too large: {} bytes", body.len());
    }
    writer
        .write_all(&(body.len() as u32).to_le_bytes())
        .await
        .context("write frame length")?;
    writer.write_all(&body).await.context("write frame body")?;
    Ok(())
}

/// Read one length-prefixed JSON frame.
pub async fn read_frame<R, T>(reader: &mut R) -> Result<Option<T>>
where
    R: AsyncRead + Unpin + ?Sized,
    T: for<'de> Deserialize<'de>,
{
    let mut len_bytes = [0u8; 4];
    let mut filled = 0usize;
    while filled < 4 {
        let n = reader
            .read(&mut len_bytes[filled..])
            .await
            .context("read frame length")?;
        if n == 0 {
            return Ok(None);
        }
        filled += n;
    }
    let len = u32::from_le_bytes(len_bytes) as usize;
    if len > MAX_FRAME {
        anyhow::bail!("frame body too large: {len} bytes");
    }
    let mut body = vec![0u8; len];
    reader
        .read_exact(&mut body)
        .await
        .context("read frame body")?;
    let value = serde_json::from_slice(&body).context("deserialize frame")?;
    Ok(Some(value))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn conversation_url_shell_defaults() {
        assert_eq!(
            parse_conversation_url("shell://powershell").unwrap(),
            ConversationSpec::Shell {
                program: "powershell".into(),
                args: vec![],
                cwd: None,
                cols: 80,
                rows: 24,
            }
        );
    }

    #[test]
    fn conversation_url_ssh_with_password() {
        assert_eq!(
            parse_conversation_url("ssh://dev@box?password=secret&cols=90&rows=28").unwrap(),
            ConversationSpec::Ssh {
                target: "dev@box".into(),
                identity: None,
                password: Some("secret".into()),
                cols: 90,
                rows: 28,
            }
        );
    }

    #[tokio::test]
    async fn frames_roundtrip() {
        let (mut client, mut server) = tokio::io::duplex(4096);
        write_frame(&mut client, &Request::Ping)
            .await
            .expect("write request");
        let request: Request = read_frame(&mut server)
            .await
            .expect("read request")
            .expect("request frame");
        assert!(matches!(request, Request::Ping));
    }
}
