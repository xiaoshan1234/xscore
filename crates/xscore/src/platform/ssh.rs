//! L0 SSH 传输：russh 建立会话 channel，提供 PTY 感知的远程 shell。

use crate::platform::Size;
use crate::{Error, Result};
use async_trait::async_trait;
use russh::client::{self, Config, Handler};
use russh::{Channel, ChannelMsg};
use std::sync::Arc;
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver};
use tokio::sync::{oneshot, Mutex};

/// russh 客户端事件处理器；当前接受服务端提供的主机密钥。
#[derive(Clone, Default)]
pub(crate) struct SshHandler;

#[async_trait]
impl Handler for SshHandler {
    type Error = russh::Error;

    async fn check_server_key(
        &mut self,
        _server_public_key: &russh_keys::key::PublicKey,
    ) -> std::result::Result<bool, Self::Error> {
        // 当前阶段接受任意 host key；known_hosts 校验后续再做。
        Ok(true)
    }
}

/// 单条 russh SSH 连接及其 PTY shell channel。
///
/// 连接创建后由一个异步任务独占 channel，并通过命令队列串行处理读写、resize
/// 和关闭。
pub struct SshConnection {
    user: String,
    host: String,
    port: u16,
    identity: Option<String>,
    password: Option<String>,
    env: Vec<(String, String)>,
    handle: Arc<Mutex<Option<client::Handle<SshHandler>>>>,
    commands: Arc<Mutex<Option<tokio::sync::mpsc::UnboundedSender<SshCommand>>>>,
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

/// 建立并认证一条 russh 客户端连接。
///
/// 参数/输出：`user`、`host`、`port`、`identity` 和 `password` 为输入；
/// 返回已完成认证、可用于打开 session channel 的客户端句柄。
pub(crate) async fn connect_authenticated(
    user: &str,
    host: &str,
    port: u16,
    identity: Option<&str>,
    password: Option<&str>,
) -> Result<client::Handle<SshHandler>> {
    let key_arc = match identity {
        Some(path) => {
            let key = russh_keys::load_secret_key(path, None)
                .map_err(|e| Error::Transport(format!("ssh key {path}: {e}")))?;
            Some(Arc::new(key))
        }
        None => None,
    };

    let config = Arc::new(Config::default());
    let mut handle = client::connect(config, (host.to_string(), port), SshHandler)
        .await
        .map_err(|e| Error::Transport(format!("ssh connect {host}:{port}: {e}")))?;

    let auth_ok = match key_arc {
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
                    "ssh: provide identity= or password for {user}@{host}"
                )));
            }
        },
    };
    if !auth_ok {
        return Err(Error::Transport(format!(
            "ssh: authentication failed for {user}@{host}"
        )));
    }

    Ok(handle)
}

impl SshConnection {
    /// 创建 SSH 连接配置，但暂不建立网络连接。
    ///
    /// 参数/输出：`user`、`host`、`port`、`identity`、`password` 和 `env`
    /// 为输入；返回尚未连接的 [`SshConnection`]。
    pub fn new(
        user: String,
        host: String,
        port: u16,
        identity: Option<String>,
        password: Option<String>,
        env: Vec<(String, String)>,
    ) -> Self {
        Self {
            user,
            host,
            port,
            identity,
            password,
            env,
            handle: Arc::new(Mutex::new(None)),
            commands: Arc::new(Mutex::new(None)),
        }
    }

    /// 连接、认证、申请 PTY 与 shell，返回远程输出字节流。
    ///
    /// 参数/输出：`self` 为输入输出，会保存连接和命令通道；`size` 为输入初始
    /// 终端尺寸；返回远程输出接收端。
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
        let handle = connect_authenticated(
            &self.user,
            &self.host,
            self.port,
            self.identity.as_deref(),
            self.password.as_deref(),
        )
        .await?;

        let mut channel = handle
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

        channel
            .request_pty(
                true,
                "xterm-256color",
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

        *self.handle.lock().await = Some(handle);
        let (out_tx, out_rx) = unbounded_channel::<Vec<u8>>();
        let (command_tx, command_rx) = unbounded_channel();
        *self.commands.lock().await = Some(command_tx);
        tokio::spawn(async move {
            run_channel(channel, command_rx, out_tx).await;
        });

        if !rejected_env.is_empty() {
            let command = environment_export_command(&rejected_env);
            self.write(command.as_bytes()).await?;
        }

        Ok(out_rx)
    }

    /// 向远端 shell 写入原始字节。
    ///
    /// 参数/输出：`self` 为输入；`data` 为输入字节；返回成功写入的字节数。
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

    /// 通知远端调整 PTY 尺寸。
    ///
    /// 参数/输出：`self` 为输入；`size` 为输入尺寸；成功时无返回值。
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

    /// 关闭 SSH channel 和客户端连接。
    ///
    /// 参数/输出：`self` 为输入输出，会清空命令通道并释放连接状态；成功时无返回值。
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
    out_tx: tokio::sync::mpsc::UnboundedSender<Vec<u8>>,
) {
    loop {
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
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{environment_export_command, is_valid_environment_name};

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
}
