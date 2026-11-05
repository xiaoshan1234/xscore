//! LocalDomain：1:1 域，一个 pane 对应一个本地 PTY 子进程。

use crate::domain::{Domain, PaneConfig, PaneHandle, PaneId, PaneKind};
use crate::platform::{LocalPty, Size};
use crate::{Error, Result};
use async_trait::async_trait;
use std::sync::Arc;
use tokio::sync::broadcast;

/// 本地 PTY 域。
///
/// 每次创建 pane 都会启动一个独立子进程，并维护其输入输出通道。
#[derive(Debug, Default)]
pub struct LocalDomain;

impl LocalDomain {
    /// 创建一个无内部状态的本地域。
    ///
    /// 参数/输出：无输入；返回新的 [`LocalDomain`]。
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Domain for LocalDomain {
    fn kind(&self) -> PaneKind {
        PaneKind::Local
    }

    async fn spawn_pane(&self, config: &PaneConfig) -> Result<Arc<dyn PaneHandle>> {
        let PaneConfig::Local {
            command,
            args,
            cwd,
            cols,
            rows,
        } = config
        else {
            return Err(Error::Invalid(
                "local domain requires Local pane config".into(),
            ));
        };
        let id = PaneId::next();
        let pty = Arc::new(LocalPty::new(command.clone(), args.clone(), cwd.clone()));
        let mut rx = pty.spawn(Size::new(*cols, *rows))?;

        let (tx, _) = broadcast::channel::<Vec<u8>>(256);
        let tx_pump = tx.clone();
        tokio::spawn(async move {
            while let Some(chunk) = rx.recv().await {
                if tx_pump.send(chunk).is_err() {
                    break;
                }
            }
        });

        Ok(Arc::new(LocalPane {
            id,
            pty,
            output: tx,
        }))
    }

    async fn close(&self) -> Result<()> {
        // 各 pane 自行关闭；LocalDomain 不额外持有连接。
        Ok(())
    }
}

/// 本地 PTY 窗格句柄。
///
/// 实现 [`PaneHandle`] 的本地字节读写、尺寸调整和进程关闭。
pub struct LocalPane {
    id: PaneId,
    pty: Arc<LocalPty>,
    output: broadcast::Sender<Vec<u8>>,
}

#[async_trait]
impl PaneHandle for LocalPane {
    fn id(&self) -> PaneId {
        self.id
    }

    fn kind(&self) -> PaneKind {
        PaneKind::Local
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    async fn send_input(&self, data: &[u8]) -> Result<usize> {
        self.pty.write(data)
    }

    async fn resize(&self, size: Size) -> Result<()> {
        self.pty.resize(size)
    }

    fn subscribe(&self) -> broadcast::Receiver<Vec<u8>> {
        self.output.subscribe()
    }

    async fn close(&self) -> Result<()> {
        self.pty.close()
    }
}
