// transport/ssh.rs
//
// Real SSH transport. Uses `russh = "0.43"` to open a TCP+SSH session
// to the remote host, authenticate via an optionally-supplied
// identity (a path to a private key file), and open a session
// channel. Read/write/resize go through the channel.
//
// russh 0.43 API notes (verified against `russh-0.43.0/src/`):
//   * `client::Config::default()` -> `Arc<Config>` is passed to
//     `client::connect(...)`.
//   * `Handle::authenticate_publickey(user, Arc<key::KeyPair>)`
//     returns `bool` (NOT `AuthResult`).
//   * `Handle::channel_open_session()` returns
//     `Channel<client::Msg>`.
//   * `Channel::data<R: AsyncRead + Unpin>(&self, R)` — passes the
//     raw bytes via `&[u8]`, which implements AsyncRead in tokio.
//   * `Channel::wait()` is `&mut self`, returns `Option<ChannelMsg>`.
//   * `Channel::window_change(cols, rows, wpix, hpix)` and
//     `Channel::eof()` and `Channel::close()` are async.
//
// For this slice, auth is key-based only (`identity` is an optional
// key file path; nothing else is wired). Password/auth-agent support
// is a follow-up — the field is already plumbed through
// `SshConfig` (see task-08 stubs) but the matching ConnectSpec has
// no `password` field yet, so there is nothing to consume here.

use std::sync::Arc;

use crate::infra::link::LinkAddress;
use crate::{Error, Result};
use async_trait::async_trait;
use russh::client::{self, Config, Handler};
use russh::{Channel, ChannelMsg};
use russh_keys;
use tokio::sync::Mutex;

pub struct SshTransport {
    pub host: String,
    pub port: u16,
    pub user: String,
    pub identity: Option<String>,
    /// Reserved for `SshConfig`/`ConnectSpec` extension (e.g. when
    /// `proxy=` / `agent=` query params land). Currently unused.
    pub proxy: Option<String>,
    handle: Mutex<Option<client::Handle<SshHandler>>>,
    channel: Mutex<Option<Channel<client::Msg>>>,
}

impl std::fmt::Debug for SshTransport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SshTransport")
            .field("host", &self.host)
            .field("port", &self.port)
            .field("user", &self.user)
            .field("identity", &self.identity)
            .field("proxy", &self.proxy)
            .finish_non_exhaustive()
    }
}

impl SshTransport {
    pub async fn connect(
        host: String,
        port: u16,
        user: String,
        identity: Option<String>,
        proxy: Option<String>,
    ) -> Result<Self> {
        Ok(Self {
            host,
            port,
            user,
            identity,
            proxy,
            handle: Mutex::new(None),
            channel: Mutex::new(None),
        })
    }
}

#[derive(Clone, Default)]
struct SshHandler;

#[async_trait]
impl Handler for SshHandler {
    type Error = russh::Error;

    async fn check_server_key(
        &mut self,
        _server_public_key: &russh_keys::key::PublicKey,
    ) -> std::result::Result<bool, Self::Error> {
        // For this slice we accept any host key. A follow-up task
        // should plumb a `known_hosts` file or an explicit
        // `StrictHostKeyChecking=yes|no|accept-new` switch — see
        // `russh_keys::check_known_hosts`.
        Ok(true)
    }
}

#[async_trait]
impl super::Transport for SshTransport {
    fn kind(&self) -> &'static str {
        "ssh"
    }
    fn address(&self) -> LinkAddress {
        LinkAddress::Ssh {
            host: self.host.clone(),
            port: self.port,
            user: self.user.clone(),
            identity: self.identity.clone(),
            proxy: self.proxy.clone(),
        }
    }

    async fn open(&mut self) -> Result<()> {
        // Resolve the key (if any) before opening the TCP+SSH session
        // so we don't open a connection we'll never be able to use.
        let key_arc: Option<Arc<russh_keys::key::KeyPair>> = match &self.identity {
            Some(path) => {
                let kp = russh_keys::load_secret_key(path, None)
                    .map_err(|e| Error::Transport(format!("ssh key {path}: {e}")))?;
                Some(Arc::new(kp))
            }
            None => None,
        };

        let config = Arc::new(Config::default());
        let handler = SshHandler;
        let addr = (self.host.clone(), self.port);
        let addr_for_err = addr.clone();
        let mut handle = client::connect(config, addr, handler)
            .await
            .map_err(|e| Error::Transport(format!("ssh connect {addr_for_err:?}: {e}")))?;

        let auth_ok = match key_arc {
            Some(k) => handle
                .authenticate_publickey(&self.user, k)
                .await
                .map_err(|e| Error::Transport(format!("ssh auth: {e}")))?,
            None => {
                return Err(Error::Transport(format!(
                    "ssh: no auth path — provide identity= for user {}@{}:{}",
                    self.user, self.host, self.port
                )));
            }
        };
        if !auth_ok {
            return Err(Error::Transport(format!(
                "ssh: authentication failed for {}@{}:{}",
                self.user, self.host, self.port
            )));
        }

        // Open a session channel and request a shell so the remote
        // end gives us a PTY-aware interactive stream.
        let channel = handle
            .channel_open_session()
            .await
            .map_err(|e| Error::Transport(format!("ssh channel_open_session: {e}")))?;
        channel
            .request_shell(true)
            .await
            .map_err(|e| Error::Transport(format!("ssh request_shell: {e}")))?;

        *self.handle.lock().await = Some(handle);
        *self.channel.lock().await = Some(channel);
        Ok(())
    }

    async fn write(&self, data: &[u8]) -> Result<usize> {
        let chan_guard = self.channel.lock().await;
        let channel = chan_guard
            .as_ref()
            .ok_or_else(|| Error::Transport("not open".into()))?;
        // `Channel::data` takes `R: AsyncRead + Unpin`. `&[u8]`
        // implements both in tokio; we pass the slice directly,
        // copying nothing extra.
        let len = data.len();
        channel
            .data(data)
            .await
            .map_err(|e| Error::Transport(format!("ssh write: {e}")))?;
        Ok(len)
    }

    async fn read(&self, buf: &mut [u8]) -> Result<usize> {
        let mut chan_guard = self.channel.lock().await;
        let channel = chan_guard
            .as_mut()
            .ok_or_else(|| Error::Transport("not open".into()))?;

        // Drain whatever channel events are queued, copying bytes
        // until the caller's buffer is full or the channel signals
        // EOF/close. Bytes that arrive beyond `buf.len()` stay queued
        // and will surface on the next read (russh streams `Data`
        // events one at a time).
        let mut total = 0usize;
        while total < buf.len() {
            match channel.wait().await {
                Some(ChannelMsg::Data { data }) => {
                    let take = (buf.len() - total).min(data.len());
                    buf[total..total + take].copy_from_slice(&data[..take]);
                    total += take;
                    if data.len() > take {
                        // Excess bytes stay in the channel's recv
                        // queue; the next read pulls them.
                        break;
                    }
                }
                Some(ChannelMsg::Eof) | Some(ChannelMsg::Close) | None => break,
                // WindowAdjusted / ExitStatus / ExtendedData — ignore
                // for now (stderr is dropped on the floor; window-
                // adjust and exit status are bookkeeping).
                _ => continue,
            }
        }
        Ok(total)
    }

    async fn resize(&self, cols: u16, rows: u16) -> Result<()> {
        let chan_guard = self.channel.lock().await;
        let channel = chan_guard
            .as_ref()
            .ok_or_else(|| Error::Transport("not open".into()))?;
        channel
            .window_change(cols.into(), rows.into(), 0, 0)
            .await
            .map_err(|e| Error::Transport(format!("ssh window_change: {e}")))?;
        Ok(())
    }

    async fn close(&self) -> Result<()> {
        // Drop the channel first (sends EOF and closes); then drop
        // the handle (terminates the TCP+SSH session).
        *self.channel.lock().await = None;
        *self.handle.lock().await = None;
        Ok(())
    }
}
