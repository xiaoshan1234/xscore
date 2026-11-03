//! L0 SSH 传输：russh 建立会话 channel，提供 PTY 感知的远程 shell。

use crate::platform::Size;
use crate::{Error, Result};
use async_trait::async_trait;
use russh::client::{self, Config, Handler};
use russh::{Channel, ChannelMsg};
use std::sync::Arc;
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver};
use tokio::sync::{oneshot, Mutex};

#[derive(Clone, Default)]
struct SshHandler;

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

pub struct SshConnection {
    user: String,
    host: String,
    port: u16,
    identity: Option<String>,
    password: Option<String>,
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
            .finish_non_exhaustive()
    }
}

impl SshConnection {
    pub fn new(
        user: String,
        host: String,
        port: u16,
        identity: Option<String>,
        password: Option<String>,
    ) -> Self {
        Self {
            user,
            host,
            port,
            identity,
            password,
            handle: Arc::new(Mutex::new(None)),
            commands: Arc::new(Mutex::new(None)),
        }
    }

    /// 连接、认证、申请 PTY 与 shell，返回远程输出字节流。
    pub async fn connect(&self, size: Size) -> Result<UnboundedReceiver<Vec<u8>>> {
        tracing::debug!(
            host = %self.host,
            port = self.port,
            user = %self.user,
            "ssh connect"
        );
        let key_arc = match &self.identity {
            Some(path) => {
                let key = russh_keys::load_secret_key(path, None)
                    .map_err(|e| Error::Transport(format!("ssh key {path}: {e}")))?;
                Some(Arc::new(key))
            }
            None => None,
        };

        let config = Arc::new(Config::default());
        let mut handle = client::connect(config, (self.host.clone(), self.port), SshHandler)
            .await
            .map_err(|e| {
                Error::Transport(format!("ssh connect {}:{}: {e}", self.host, self.port))
            })?;

        let auth_ok = match key_arc {
            Some(key) => handle
                .authenticate_publickey(&self.user, key)
                .await
                .map_err(|e| Error::Transport(format!("ssh auth: {e}")))?,
            None => match &self.password {
                Some(password) => handle
                    .authenticate_password(&self.user, password.clone())
                    .await
                    .map_err(|e| Error::Transport(format!("ssh auth: {e}")))?,
                None => {
                    return Err(Error::Transport(format!(
                        "ssh: provide identity= or password for {}@{}",
                        self.user, self.host
                    )));
                }
            },
        };
        if !auth_ok {
            return Err(Error::Transport(format!(
                "ssh: authentication failed for {}@{}",
                self.user, self.host
            )));
        }

        let channel = handle
            .channel_open_session()
            .await
            .map_err(|e| Error::Transport(format!("ssh channel_open_session: {e}")))?;
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

        Ok(out_rx)
    }

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
