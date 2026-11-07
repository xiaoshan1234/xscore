//! daemon 命令模型与输入解析：可分发命令以及 URL/SSH 参数解析。

use std::collections::HashMap;
use std::path::PathBuf;

use anyhow::{anyhow, Result};

/// daemon 内分发的命令。
#[derive(Debug, Clone)]
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
#[derive(Debug, Clone)]
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
#[derive(Debug, Clone)]
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
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogFormat {
    /// Record raw terminal bytes.
    Raw,
    /// Record ANSI-filtered timestamped lines.
    Char,
}

/// Parameters for recording pane output.
#[derive(Debug, Clone)]
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
#[derive(Debug, Clone)]
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
#[derive(Debug, Clone)]
pub struct TmxAttachRequest {
    /// tmux URL.
    pub url: String,
    /// Optional SSH endpoint URL.
    pub ssh: Option<String>,
    /// Optional WSL endpoint URL.
    pub wsl: Option<String>,
}

/// `ls` command payloads.
#[derive(Debug, Clone)]
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
}
