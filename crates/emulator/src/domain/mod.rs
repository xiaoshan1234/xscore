//! L1 域/连接抽象：把“连接”抽象为可插拔 Domain，产出统一 PaneHandle。

pub mod local;

use crate::platform::Size;
use crate::Result;
use async_trait::async_trait;
use std::sync::Arc;
use tokio::sync::broadcast;

pub use local::{LocalDomain, LocalPane};

/// 逻辑窗格句柄标识。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PaneId(u64);

impl PaneId {
    pub fn new(n: u64) -> Self {
        Self(n)
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

/// 新建 pane 的配置。
#[derive(Debug, Clone)]
pub struct PaneConfig {
    pub command: String,
    pub args: Vec<String>,
    pub cwd: Option<String>,
    pub cols: u16,
    pub rows: u16,
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
