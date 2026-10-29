//! L1 域/连接抽象：把“连接”抽象为可插拔 Domain，产出统一 PaneHandle。

pub mod local;
pub mod ssh;
pub mod tmux;

use crate::platform::Size;
use crate::Result;
use async_trait::async_trait;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tokio::sync::broadcast;

pub use local::{LocalDomain, LocalPane};
pub use ssh::{RemoteSshDomain, SshPane};
pub use tmux::{TmuxDomain, TmuxPane};

/// 逻辑窗格句柄标识。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PaneId(u64);

impl PaneId {
    pub fn new(n: u64) -> Self {
        Self(n)
    }

    /// 进程内全局唯一的下一个 pane id。
    pub fn next() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        Self(NEXT.fetch_add(1, Ordering::Relaxed) + 1)
    }

    pub fn as_u64(&self) -> u64 {
        self.0
    }
}

impl std::fmt::Display for PaneId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// 后端类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaneKind {
    Local,
    Ssh,
    Tmux,
}

/// 新建 pane 的配置：按后端类型区分。
#[derive(Debug, Clone)]
pub enum PaneConfig {
    Local {
        command: String,
        args: Vec<String>,
        cwd: Option<String>,
        cols: u16,
        rows: u16,
    },
    Ssh {
        host: String,
        port: u16,
        user: String,
        identity: Option<String>,
        password: Option<String>,
        cols: u16,
        rows: u16,
    },
}

#[async_trait]
pub trait PaneHandle: Send + Sync {
    fn id(&self) -> PaneId;
    fn kind(&self) -> PaneKind;
    async fn send_input(&self, data: &[u8]) -> Result<usize>;
    async fn resize(&self, size: Size) -> Result<()>;
    fn subscribe(&self) -> broadcast::Receiver<Vec<u8>>;
    async fn close(&self) -> Result<()>;
}

#[async_trait]
pub trait Domain: Send + Sync {
    fn kind(&self) -> PaneKind;
    async fn spawn_pane(&self, config: &PaneConfig) -> Result<Arc<dyn PaneHandle>>;
    async fn close(&self) -> Result<()>;
}
