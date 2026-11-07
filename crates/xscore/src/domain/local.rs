//! LocalDomain：1:1 域，一个 pane 对应一个本地 PTY 子进程。

use crate::domain::{Domain, TerminalConfig, TerminalHandle, TerminalId, TerminalKind};
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
    fn kind(&self) -> TerminalKind {
        TerminalKind::Local
    }

    async fn spawn_terminal(&self, config: &TerminalConfig) -> Result<Arc<dyn TerminalHandle>> {
        let TerminalConfig::Local {
            command,
            args,
            cwd,
            env,
            cols,
            rows,
        } = config
        else {
            return Err(Error::Invalid(
                "local domain requires Local pane config".into(),
            ));
        };
        let id = TerminalId::next();
        let pty = Arc::new(LocalPty::new(
            command.clone(),
            args.clone(),
            cwd.clone(),
            env.clone(),
        ));
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

        Ok(Arc::new(LocalTerminal {
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

/// 本地 PTY 终端句柄。
///
/// 实现 [`TerminalHandle`] 的本地字节读写、尺寸调整和进程关闭。
pub struct LocalTerminal {
    id: TerminalId,
    pty: Arc<LocalPty>,
    output: broadcast::Sender<Vec<u8>>,
}

#[async_trait]
impl TerminalHandle for LocalTerminal {
    fn id(&self) -> TerminalId {
        self.id
    }

    fn kind(&self) -> TerminalKind {
        TerminalKind::Local
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
