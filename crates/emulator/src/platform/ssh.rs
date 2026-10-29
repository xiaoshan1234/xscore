//! L0 SSH 传输：russh 建立会话 channel，提供 PTY 感知的远程 shell。

use crate::platform::Size;
use crate::{Error, Result};
use async_trait::async_trait;
use russh::client::{self, Config, Handler};
use russh::{Channel, ChannelMsg};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver};
use tokio::sync::Mutex;

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
    handle: Arc<Mutex<Option<client::Handle<SshHandler>>>>,
    channel: Arc<Mutex<Option<Channel<client::Msg>>>>,
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
    pub fn new(user: String, host: String, port: u16, identity: Option<String>) -> Self {
        Self {
            user,
            host,
            port,
            identity,
            handle: Arc::new(Mutex::new(None)),
            channel: Arc::new(Mutex::new(None)),
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
            None => {
                return Err(Error::Transport(format!(
                    "ssh: identity= is required for {}@{}",
                    self.user, self.host
                )));
            }
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
        *self.channel.lock().await = Some(channel);

        let (out_tx, out_rx) = unbounded_channel::<Vec<u8>>();
        let channel = self.channel.clone();
        tokio::spawn(async move {
            loop {
                // `wait` 独占 &mut channel；用 10ms 超时周期性释放锁，
                // 让 write/resize 有机会拿到 channel 发送数据。
                let msg = {
                    let mut guard = channel.lock().await;
                    match guard.as_mut() {
                        Some(ch) => tokio::time::timeout(Duration::from_millis(10), ch.wait())
                            .await
                            .ok()
                            .flatten(),
                        None => None,
                    }
                };
                match msg {
                    Some(ChannelMsg::Data { data }) => {
                        if out_tx.send(data.to_vec()).is_err() {
                            break;
                        }
                    }
                    Some(ChannelMsg::Eof) | Some(ChannelMsg::Close) | None => break,
                    Some(_) => continue,
                }
            }
        });

        Ok(out_rx)
    }

    pub async fn write(&self, data: &[u8]) -> Result<usize> {
        let guard = self.channel.lock().await;
        let channel = guard
            .as_ref()
            .ok_or_else(|| Error::Transport("ssh: not connected".into()))?;
        let len = data.len();
        channel
            .data(data)
            .await
            .map_err(|e| Error::Transport(format!("ssh write: {e}")))?;
        Ok(len)
    }

    pub async fn resize(&self, size: Size) -> Result<()> {
        let guard = self.channel.lock().await;
        let channel = guard
            .as_ref()
            .ok_or_else(|| Error::Transport("ssh: not connected".into()))?;
        channel
            .window_change(size.cols as u32, size.rows as u32, 0, 0)
            .await
            .map_err(|e| Error::Transport(format!("ssh window_change: {e}")))?;
        Ok(())
    }

    pub async fn close(&self) -> Result<()> {
        *self.channel.lock().await = None;
        *self.handle.lock().await = None;
        Ok(())
    }
}
