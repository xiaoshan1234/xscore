//! L0 SSH transport: establish a russh session channel and provide a
//! PTY-aware remote shell.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use russh::client::{self, Config, Handler};
use russh::{Channel, ChannelMsg};
use tokio::net::{TcpSocket, TcpStream};
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender};
use tokio::sync::{oneshot, Mutex};

use crate::platform::Size;
use crate::{Error, Result};

/// Interval used by the application-layer null-packet keepalive.
const NULL_PACKET_KEEPALIVE_SECS: u64 = 60;

/// Default terminal type requested from the SSH server.
const DEFAULT_TERMINAL_TYPE: &str = "xterm-256color";

/// Optional connection-level knobs for the SSH transport.
///
/// Every field is optional or boolean-defaulted so `SshOptions::default()`
/// preserves the historical behaviour (no keepalive, direct connection,
/// host keys accepted, `xterm-256color`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SshOptions {
    /// Send an SSH-level keepalive when nothing has been received for this
    /// long. `None` disables protocol keepalives.
    pub keepalive_interval: Option<Duration>,
    /// Maximum time to wait for the TCP connect plus SSH handshake.
    /// `None` means no explicit timeout.
    pub connection_timeout: Option<Duration>,
    /// Set `TCP_NODELAY` on the socket. Defaults to `true` when `None`.
    pub tcp_nodelay: Option<bool>,
    /// Set `SO_KEEPALIVE` on the socket. Defaults to `false` when `None`.
    pub so_keepalive: Option<bool>,
    /// Prefer SSH compression (zlib) during negotiation.
    pub enable_compression: bool,
    /// Send a zero byte through the channel every 60 seconds to keep the
    /// application layer alive.
    pub null_packet_keepalive: bool,
    /// `ProxyJump` target in `[user@]host[:port]` form. The jump host is
    /// authenticated with the same identity/password as the target.
    pub proxy_jump: Option<String>,
    /// Path to a `known_hosts` file. When set, the server key is verified
    /// against it; unknown or changed keys are rejected. `None` accepts any
    /// key (host-key verification disabled).
    pub known_hosts_path: Option<String>,
    /// Terminal type requested via `pty-req`. Defaults to `xterm-256color`.
    pub term_type: Option<String>,
}

/// A russh client handler. When [`SshOptions::known_hosts_path`] is set the
/// server key is verified against that file; otherwise any key is accepted.
#[derive(Clone, Default)]
pub(crate) struct SshHandler {
    known_hosts_path: Option<String>,
    host: String,
    port: u16,
}

#[async_trait]
impl Handler for SshHandler {
    type Error = russh::Error;

    async fn check_server_key(
        &mut self,
        server_public_key: &russh_keys::key::PublicKey,
    ) -> std::result::Result<bool, Self::Error> {
        let Some(path) = self.known_hosts_path.as_deref() else {
            return Ok(true);
        };

        match russh_keys::check_known_hosts_path(&self.host, self.port, server_public_key, path) {
            Ok(true) => Ok(true),
            Ok(false) => {
                tracing::warn!(
                    host = %self.host,
                    port = self.port,
                    "ssh host key not found in known_hosts; rejecting connection"
                );
                Ok(false)
            }
            Err(e) => {
                tracing::warn!(
                    host = %self.host,
                    port = self.port,
                    error = %e,
                    "ssh host key verification failed; rejecting connection"
                );
                Ok(false)
            }
        }
    }
}

/// An established client handle plus the optional proxy-jump connection that
/// must stay alive for as long as the target handle does.
pub(crate) struct SshClient {
    /// Handle for the target connection.
    pub(crate) handle: client::Handle<SshHandler>,
    /// Proxy-jump connection kept alive for the same lifetime as `handle`.
    #[allow(dead_code)]
    pub(crate) jump: Option<client::Handle<SshHandler>>,
}

/// Parsed `ProxyJump` value.
struct ParsedProxyJump {
    user: Option<String>,
    host: String,
    port: Option<u16>,
}

/// Parse `[user@]host[:port]` into its components. Returns `None` for empty
/// input, an empty user/host component, whitespace inside the host, or a
/// non-numeric port suffix.
fn parse_proxy_jump(raw: &str) -> Option<ParsedProxyJump> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }

    let (user, rest) = match trimmed.find('@') {
        Some(idx) if idx > 0 => (Some(trimmed[..idx].trim()), trimmed[idx + 1..].trim()),
        Some(_) => return None,
        None => (None, trimmed),
    };
    if rest.is_empty() {
        return None;
    }

    let (host, port) = match rest.rfind(':') {
        Some(idx) => {
            let port_str = rest[idx + 1..].trim();
            let host_part = rest[..idx].trim();
            if !port_str.is_empty() && port_str.bytes().all(|b| b.is_ascii_digit()) {
                (host_part, port_str.parse::<u16>().ok())
            } else {
                (rest, None)
            }
        }
        None => (rest, None),
    };

    if host.is_empty() || host.chars().any(|c| c.is_whitespace()) {
        return None;
    }

    Some(ParsedProxyJump {
        user: user.filter(|u| !u.is_empty()).map(str::to_string),
        host: host.to_string(),
        port,
    })
}

/// Build a russh client config from the supplied options.
fn build_client_config(options: &SshOptions) -> Arc<Config> {
    let mut config = Config::default();
    if let Some(keepalive) = options.keepalive_interval {
        config.keepalive_interval = Some(keepalive);
    }
    if options.enable_compression {
        config.preferred = russh::Preferred::COMPRESSED;
    }
    Arc::new(config)
}

/// Open a TCP connection to `host:port`, applying the configured socket
/// options and an optional connection timeout.
async fn open_tcp_stream(host: &str, port: u16, options: &SshOptions) -> Result<TcpStream> {
    let nodelay = options.tcp_nodelay.unwrap_or(true);
    let keepalive = options.so_keepalive.unwrap_or(false);

    let mut addrs: Vec<SocketAddr> = tokio::net::lookup_host((host, port))
        .await
        .map_err(|e| Error::Transport(format!("ssh dns lookup {host}:{port}: {e}")))?
        .collect();
    if addrs.is_empty() {
        return Err(Error::Transport(format!(
            "no addresses found for {host}:{port}"
        )));
    }

    // Prefer IPv4 to preserve stable address ordering.
    addrs.sort_by_key(|addr| match addr {
        SocketAddr::V4(_) => 0,
        SocketAddr::V6(_) => 1,
    });

    let mut last_error = None;
    for addr in addrs {
        let socket = match addr {
            SocketAddr::V4(_) => TcpSocket::new_v4(),
            SocketAddr::V6(_) => TcpSocket::new_v6(),
        };
        let socket = match socket {
            Ok(socket) => socket,
            Err(e) => {
                last_error = Some(e.to_string());
                continue;
            }
        };

        if let Err(e) = socket.set_nodelay(nodelay) {
            tracing::warn!(address = %addr, error = %e, "ssh set_nodelay failed");
        }
        if let Err(e) = socket.set_keepalive(keepalive) {
            tracing::warn!(address = %addr, error = %e, "ssh set_keepalive failed");
        }

        match socket.connect(addr).await {
            Ok(stream) => return Ok(stream),
            Err(e) => last_error = Some(e.to_string()),
        }
    }

    Err(Error::Transport(format!(
        "ssh connect to {host}:{port} failed: {}",
        last_error.unwrap_or_else(|| "no addresses succeeded".to_string())
    )))
}

/// Load a private key from `path`. Passphrases are intentionally unsupported
/// at this layer.
fn load_identity(path: &str) -> Result<Arc<russh_keys::key::KeyPair>> {
    let key = russh_keys::load_secret_key(path, None)
        .map_err(|e| Error::Transport(format!("ssh key {path}: {e}")))?;
    Ok(Arc::new(key))
}

/// Authenticate an already-connected client handle.
async fn authenticate_handle(
    handle: &mut client::Handle<SshHandler>,
    user: &str,
    key: Option<Arc<russh_keys::key::KeyPair>>,
    password: Option<&str>,
) -> Result<()> {
    let ok = match key {
        Some(key) => handle
            .authenticate_publickey(user, key)
            .await
            .map_err(|e| Error::Transport(format!("ssh auth: {e}")))?,
        None => match password {
            Some(password) => handle
                .authenticate_password(user, password.to_string())
                .await
                .map_err(|e| Error::Transport(format!("ssh auth: {e}")))?,
            None => {
                return Err(Error::Transport(format!(
                    "ssh: provide identity= or password for {user}"
                )));
            }
        },
    };

    if !ok {
        return Err(Error::Transport(format!(
            "ssh: authentication failed for {user}"
        )));
    }
    Ok(())
}

/// Connect directly to `host:port` and authenticate.
async fn connect_direct(
    user: &str,
    host: &str,
    port: u16,
    key: Option<Arc<russh_keys::key::KeyPair>>,
    password: Option<&str>,
    options: &SshOptions,
    handler: SshHandler,
) -> Result<client::Handle<SshHandler>> {
    let config = build_client_config(options);
    let connect_block = async {
        let stream = open_tcp_stream(host, port, options).await?;
        client::connect_stream(config, stream, handler)
            .await
            .map_err(|e| Error::Transport(format!("ssh connect {host}:{port}: {e}")))
    };
    let mut handle = match options.connection_timeout {
        Some(timeout) => tokio::time::timeout(timeout, connect_block)
            .await
            .map_err(|_| Error::Transport(format!("ssh connect to {host}:{port} timed out")))??,
        None => connect_block.await?,
    };
    authenticate_handle(&mut handle, user, key, password).await?;
    Ok(handle)
}

/// Establish and authenticate a russh client connection, optionally routing
/// the connection through a `ProxyJump` host.
pub(crate) async fn connect_authenticated(
    user: &str,
    host: &str,
    port: u16,
    identity: Option<&str>,
    password: Option<&str>,
    options: &SshOptions,
) -> Result<SshClient> {
    let key = match identity {
        Some(path) => Some(load_identity(path)?),
        None => None,
    };

    if let Some(raw) = options.proxy_jump.as_deref() {
        let jump = parse_proxy_jump(raw)
            .ok_or_else(|| Error::Invalid(format!("invalid proxy_jump: {raw:?}")))?;
        let jump_port = jump.port.unwrap_or(22);
        let jump_user = jump.user.as_deref().unwrap_or(user);
        let jump_handler = SshHandler {
            known_hosts_path: options.known_hosts_path.clone(),
            host: jump.host.clone(),
            port: jump_port,
        };
        let jump_handle = connect_direct(
            jump_user,
            &jump.host,
            jump_port,
            key.clone(),
            password,
            options,
            jump_handler,
        )
        .await?;

        let channel = jump_handle
            .channel_open_direct_tcpip(host, port as u32, "127.0.0.1", 0)
            .await
            .map_err(|e| Error::Transport(format!("ssh proxy channel to {host}:{port}: {e}")))?;
        let stream = channel.into_stream();

        let target_handler = SshHandler {
            known_hosts_path: options.known_hosts_path.clone(),
            host: host.to_string(),
            port,
        };
        let config = build_client_config(options);
        let target_connect = async {
            client::connect_stream(config, stream, target_handler)
                .await
                .map_err(|e| {
                    Error::Transport(format!("ssh connect via proxy to {host}:{port}: {e}"))
                })
        };
        let mut handle = match options.connection_timeout {
            Some(timeout) => tokio::time::timeout(timeout, target_connect)
                .await
                .map_err(|_| {
                    Error::Transport(format!("ssh connect to {host}:{port} timed out"))
                })??,
            None => target_connect.await?,
        };
        authenticate_handle(&mut handle, user, key, password).await?;

        Ok(SshClient {
            handle,
            jump: Some(jump_handle),
        })
    } else {
        let handler = SshHandler {
            known_hosts_path: options.known_hosts_path.clone(),
            host: host.to_string(),
            port,
        };
        let handle = connect_direct(user, host, port, key, password, options, handler).await?;
        Ok(SshClient { handle, jump: None })
    }
}

/// A single russh SSH connection and its PTY shell channel.
pub struct SshConnection {
    user: String,
    host: String,
    port: u16,
    identity: Option<String>,
    password: Option<String>,
    env: Vec<(String, String)>,
    options: SshOptions,
    handle: Arc<Mutex<Option<SshClient>>>,
    commands: Arc<Mutex<Option<UnboundedSender<SshCommand>>>>,
}

enum SshCommand {
    Write(Vec<u8>, oneshot::Sender<Result<usize>>),
    Resize(Size, oneshot::Sender<Result<()>>),
    Close(oneshot::Sender<Result<()>>),
}

impl std::fmt::Debug for SshConnection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SshConnection")
            .field("user", &self.user)
            .field("host", &self.host)
            .field("port", &self.port)
            .field(
                "env_names",
                &self.env.iter().map(|(name, _)| name).collect::<Vec<_>>(),
            )
            .finish_non_exhaustive()
    }
}

impl SshConnection {
    /// Create an SSH connection config with default options, without opening
    /// the network connection yet.
    pub fn new(
        user: String,
        host: String,
        port: u16,
        identity: Option<String>,
        password: Option<String>,
        env: Vec<(String, String)>,
    ) -> Self {
        Self::with_options(
            user,
            host,
            port,
            identity,
            password,
            env,
            SshOptions::default(),
        )
    }

    /// Create an SSH connection config with explicit connection options,
    /// without opening the network connection yet.
    pub fn with_options(
        user: String,
        host: String,
        port: u16,
        identity: Option<String>,
        password: Option<String>,
        env: Vec<(String, String)>,
        options: SshOptions,
    ) -> Self {
        Self {
            user,
            host,
            port,
            identity,
            password,
            env,
            options,
            handle: Arc::new(Mutex::new(None)),
            commands: Arc::new(Mutex::new(None)),
        }
    }

    /// Connect, authenticate, request a PTY and a shell, and return the remote
    /// output byte stream.
    pub async fn connect(&self, size: Size) -> Result<UnboundedReceiver<Vec<u8>>> {
        for (name, value) in &self.env {
            if !is_valid_environment_name(name) {
                return Err(Error::Invalid(format!(
                    "invalid SSH environment variable name: {name:?}"
                )));
            }
            if value.contains('\0') {
                return Err(Error::Invalid(format!(
                    "SSH environment variable {name:?} must not contain a NUL byte"
                )));
            }
        }

        tracing::debug!(
            host = %self.host,
            port = self.port,
            user = %self.user,
            "ssh connect"
        );
        let client = connect_authenticated(
            &self.user,
            &self.host,
            self.port,
            self.identity.as_deref(),
            self.password.as_deref(),
            &self.options,
        )
        .await?;

        let mut channel = client
            .handle
            .channel_open_session()
            .await
            .map_err(|e| Error::Transport(format!("ssh channel_open_session: {e}")))?;

        let mut rejected_env = Vec::new();
        for (name, value) in &self.env {
            channel
                .set_env(true, name.as_str(), value.as_str())
                .await
                .map_err(|e| Error::Transport(format!("ssh set_env {name}: {e}")))?;
            match channel.wait().await {
                Some(ChannelMsg::Success) => {}
                Some(ChannelMsg::Failure) => rejected_env.push((name.clone(), value.clone())),
                Some(ChannelMsg::Eof | ChannelMsg::Close) | None => {
                    return Err(Error::Transport(
                        "ssh channel closed while setting environment".into(),
                    ));
                }
                Some(_) => {}
            }
        }
        if !rejected_env.is_empty() {
            tracing::debug!(
                env_names = ?rejected_env.iter().map(|(name, _)| name).collect::<Vec<_>>(),
                "ssh server rejected environment requests; using shell fallback"
            );
        }

        let term_type = self
            .options
            .term_type
            .as_deref()
            .unwrap_or(DEFAULT_TERMINAL_TYPE);
        channel
            .request_pty(
                true,
                term_type,
                size.cols as u32,
                size.rows as u32,
                0,
                0,
                &[],
            )
            .await
            .map_err(|e| Error::Transport(format!("ssh request_pty: {e}")))?;
        channel
            .request_shell(true)
            .await
            .map_err(|e| Error::Transport(format!("ssh request_shell: {e}")))?;

        *self.handle.lock().await = Some(client);
        let (out_tx, out_rx) = unbounded_channel::<Vec<u8>>();
        let (command_tx, command_rx) = unbounded_channel();
        *self.commands.lock().await = Some(command_tx);
        let null_packet_keepalive = self.options.null_packet_keepalive;
        tokio::spawn(async move {
            run_channel(channel, command_rx, out_tx, null_packet_keepalive).await;
        });

        if !rejected_env.is_empty() {
            let command = environment_export_command(&rejected_env);
            self.write(command.as_bytes()).await?;
        }

        Ok(out_rx)
    }

    /// Write raw bytes to the remote shell.
    pub async fn write(&self, data: &[u8]) -> Result<usize> {
        let (reply, result) = oneshot::channel();
        let command_tx = self
            .commands
            .lock()
            .await
            .as_ref()
            .cloned()
            .ok_or_else(|| Error::Transport("ssh: not connected".into()))?;
        command_tx
            .send(SshCommand::Write(data.to_vec(), reply))
            .map_err(|_| Error::Transport("ssh: not connected".into()))?;
        result
            .await
            .map_err(|_| Error::Transport("ssh: command task stopped".into()))?
    }

    /// Notify the remote host to resize the PTY.
    pub async fn resize(&self, size: Size) -> Result<()> {
        let (reply, result) = oneshot::channel();
        let command_tx = self
            .commands
            .lock()
            .await
            .as_ref()
            .cloned()
            .ok_or_else(|| Error::Transport("ssh: not connected".into()))?;
        command_tx
            .send(SshCommand::Resize(size, reply))
            .map_err(|_| Error::Transport("ssh: not connected".into()))?;
        result
            .await
            .map_err(|_| Error::Transport("ssh: command task stopped".into()))?
    }

    /// Close the SSH channel and release the client connection.
    pub async fn close(&self) -> Result<()> {
        let command_tx = self.commands.lock().await.take();
        if let Some(command_tx) = command_tx {
            let (reply, result) = oneshot::channel();
            if command_tx.send(SshCommand::Close(reply)).is_ok() {
                let _ = result.await;
            }
        }
        *self.handle.lock().await = None;
        Ok(())
    }
}

fn is_valid_environment_name(name: &str) -> bool {
    let mut chars = name.chars();
    matches!(chars.next(), Some(ch) if ch == '_' || ch.is_ascii_alphabetic())
        && chars.all(|ch| ch == '_' || ch.is_ascii_alphanumeric())
}

fn quote_shell_value(value: &str) -> String {
    let mut quoted = String::with_capacity(value.len() + 2);
    quoted.push('\'');
    for ch in value.chars() {
        if ch == '\'' {
            quoted.push_str(r"'\''");
        } else {
            quoted.push(ch);
        }
    }
    quoted.push('\'');
    quoted
}

fn environment_export_command(env: &[(String, String)]) -> String {
    let mut command = String::from("export");
    for (name, value) in env {
        command.push(' ');
        command.push_str(name);
        command.push('=');
        command.push_str(&quote_shell_value(value));
    }
    command.push('\n');
    command
}

async fn run_channel(
    mut channel: Channel<client::Msg>,
    mut commands: UnboundedReceiver<SshCommand>,
    out_tx: UnboundedSender<Vec<u8>>,
    null_packet_keepalive: bool,
) {
    let mut keepalive = null_packet_keepalive.then(|| {
        let mut interval = tokio::time::interval(Duration::from_secs(NULL_PACKET_KEEPALIVE_SECS));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        interval
    });
    if let Some(interval) = keepalive.as_mut() {
        interval.tick().await;
    }

    loop {
        let keepalive_tick = async {
            match keepalive.as_mut() {
                Some(interval) => interval.tick().await,
                None => std::future::pending().await,
            }
        };

        tokio::select! {
            message = channel.wait() => {
                match message {
                    Some(ChannelMsg::Data { data }) | Some(ChannelMsg::ExtendedData { data, .. }) => {
                        if out_tx.send(data.to_vec()).is_err() {
                            break;
                        }
                    }
                    Some(ChannelMsg::Eof) | Some(ChannelMsg::Close) | None => break,
                    Some(_) => {}
                }
            }
            command = commands.recv() => {
                let Some(command) = command else {
                    break;
                };
                match command {
                    SshCommand::Write(data, reply) => {
                        let result = channel
                            .data(data.as_slice())
                            .await
                            .map(|()| data.len())
                            .map_err(|error| Error::Transport(format!("ssh write: {error}")));
                        let _ = reply.send(result);
                    }
                    SshCommand::Resize(size, reply) => {
                        let result = channel
                            .window_change(size.cols as u32, size.rows as u32, 0, 0)
                            .await
                            .map_err(|error| {
                                Error::Transport(format!("ssh window_change: {error}"))
                            });
                        let _ = reply.send(result);
                    }
                    SshCommand::Close(reply) => {
                        let result = channel
                            .close()
                            .await
                            .map_err(|error| Error::Transport(format!("ssh close: {error}")));
                        let _ = reply.send(result);
                        break;
                    }
                }
            }
            _ = keepalive_tick => {
                if channel.data(&b"\0"[..]).await.is_err() {
                    break;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{environment_export_command, is_valid_environment_name, parse_proxy_jump};

    #[test]
    fn environment_name_validation_accepts_shell_identifiers() {
        assert!(is_valid_environment_name("PATH"));
        assert!(is_valid_environment_name("_private1"));
        assert!(!is_valid_environment_name(""));
        assert!(!is_valid_environment_name("1PATH"));
        assert!(!is_valid_environment_name("BAD-NAME"));
        assert!(!is_valid_environment_name("A=B"));
    }

    #[test]
    fn export_command_quotes_values_and_exports_children() {
        assert_eq!(
            environment_export_command(&[
                ("PLAIN".into(), "value".into()),
                ("QUOTED".into(), "a'b c".into()),
            ]),
            "export PLAIN='value' QUOTED='a'\\''b c'\n"
        );
    }

    #[test]
    fn proxy_jump_parses_user_host_and_port() {
        let parsed = parse_proxy_jump("bastion@jump.example.com:2222").unwrap();
        assert_eq!(parsed.user.as_deref(), Some("bastion"));
        assert_eq!(parsed.host, "jump.example.com");
        assert_eq!(parsed.port, Some(2222));
    }

    #[test]
    fn proxy_jump_parses_host_only() {
        let parsed = parse_proxy_jump("jump.example.com").unwrap();
        assert_eq!(parsed.user, None);
        assert_eq!(parsed.host, "jump.example.com");
        assert_eq!(parsed.port, None);
    }

    #[test]
    fn proxy_jump_rejects_empty_and_malformed() {
        assert!(parse_proxy_jump("").is_none());
        assert!(parse_proxy_jump("   ").is_none());
        assert!(parse_proxy_jump("@host").is_none());
        assert!(parse_proxy_jump("user@").is_none());
        assert!(parse_proxy_jump("bad host").is_none());
    }
}
