//! RemoteSshDomain：1:1 域，一个 pane 对应一条 SSH channel。

use crate::domain::{Domain, PaneConfig, PaneHandle, PaneId, PaneKind};
use crate::platform::ssh::SshConnection;
use crate::platform::Size;
use crate::{Error, Result};
use async_trait::async_trait;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tokio::sync::broadcast;

#[derive(Debug, Default)]
pub struct RemoteSshDomain {
    next_pane: AtomicU64,
}

impl RemoteSshDomain {
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait]
impl Domain for RemoteSshDomain {
    fn kind(&self) -> PaneKind {
        PaneKind::Ssh
    }

    async fn spawn_pane(&self, config: &PaneConfig) -> Result<Arc<dyn PaneHandle>> {
        let PaneConfig::Ssh {
            host,
            port,
            user,
            identity,
            cols,
            rows,
        } = config
        else {
            return Err(Error::Invalid("ssh domain requires Ssh pane config".into()));
        };

        let id = PaneId::new(self.next_pane.fetch_add(1, Ordering::Relaxed) + 1);
        let conn = Arc::new(SshConnection::new(
            user.clone(),
            host.clone(),
            *port,
            identity.clone(),
        ));
        let mut rx = conn.connect(Size::new(*cols, *rows)).await?;

        let (tx, _) = broadcast::channel::<Vec<u8>>(256);
        let tx_pump = tx.clone();
        tokio::spawn(async move {
            while let Some(chunk) = rx.recv().await {
                if tx_pump.send(chunk).is_err() {
                    break;
                }
            }
        });

        Ok(Arc::new(SshPane {
            id,
            conn,
            output: tx,
        }))
    }

    async fn close(&self) -> Result<()> {
        Ok(())
    }
}

pub struct SshPane {
    id: PaneId,
    conn: Arc<SshConnection>,
    output: broadcast::Sender<Vec<u8>>,
}

#[async_trait]
impl PaneHandle for SshPane {
    fn id(&self) -> PaneId {
        self.id
    }

    fn kind(&self) -> PaneKind {
        PaneKind::Ssh
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
