//! RemoteSshDomain：1:1 域，一个 pane 对应一条 SSH channel。

use crate::domain::{Domain, TerminalConfig, TerminalHandle, TerminalId, TerminalKind};
use crate::platform::ssh::SshConnection;
use crate::platform::Size;
use crate::{Error, Result};
use async_trait::async_trait;
use std::sync::Arc;
use tokio::sync::broadcast;

/// russh SSH 域。
///
/// 每次创建 pane 都会建立独立 SSH 连接和 PTY shell channel。
#[derive(Debug, Default)]
pub struct RemoteSshDomain;

impl RemoteSshDomain {
    /// 创建一个无内部状态的 SSH 域。
    ///
    /// 参数/输出：无输入；返回新的 [`RemoteSshDomain`]。
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Domain for RemoteSshDomain {
    fn kind(&self) -> TerminalKind {
        TerminalKind::Ssh
    }

    async fn spawn_terminal(&self, config: &TerminalConfig) -> Result<Arc<dyn TerminalHandle>> {
        let TerminalConfig::Ssh {
            host,
            port,
            user,
            identity,
            password,
            env,
            cols,
            rows,
            options,
        } = config
        else {
            return Err(Error::Invalid("ssh domain requires Ssh pane config".into()));
        };

        let id = TerminalId::next();
        let conn = Arc::new(SshConnection::with_options(
            user.clone(),
            host.clone(),
            *port,
            identity.clone(),
            password.clone(),
            env.clone(),
            options.clone(),
        ));
        let mut rx = conn.connect(Size::new(*cols, *rows)).await?;

        let (tx, _) = broadcast::channel::<Vec<u8>>(256);
        let tx_pump = tx.clone();
        tokio::spawn(async move {
            while let Some(chunk) = rx.recv().await {
                let _ = tx_pump.send(chunk);
            }
        });

        Ok(Arc::new(SshTerminal {
            id,
            conn,
            output: tx,
        }))
    }

    async fn close(&self) -> Result<()> {
        Ok(())
    }
}

/// 单连接 SSH 终端句柄。
///
/// 输入输出直接映射到该 pane 独占的 SSH channel。
pub struct SshTerminal {
    id: TerminalId,
    conn: Arc<SshConnection>,
    output: broadcast::Sender<Vec<u8>>,
}

#[async_trait]
impl TerminalHandle for SshTerminal {
    fn id(&self) -> TerminalId {
        self.id
    }

    fn kind(&self) -> TerminalKind {
        TerminalKind::Ssh
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    async fn send_input(&self, data: &[u8]) -> Result<usize> {
        self.conn.write(data).await
    }

    async fn resize(&self, size: Size) -> Result<()> {
        self.conn.resize(size).await
    }

    fn subscribe(&self) -> broadcast::Receiver<Vec<u8>> {
        self.output.subscribe()
    }

    async fn close(&self) -> Result<()> {
        self.conn.close().await
    }
}
