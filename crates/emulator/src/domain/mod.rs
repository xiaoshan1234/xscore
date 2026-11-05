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
pub use tmux::{enumerate, make_pane, TmuxDomain, TmuxPane, TmuxPaneInfo, TmuxWindowInfo};

/// 逻辑窗格句柄标识。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PaneId(u64);

impl PaneId {
    /// 用指定整数构造 pane ID。
    ///
    /// 参数/输出：`n` 为输入 ID 数值；返回包装后的 [`PaneId`]。
    pub fn new(n: u64) -> Self {
        Self(n)
    }

    /// 分配进程内全局唯一的下一个 pane ID。
    ///
    /// 参数/输出：无显式输入；返回新分配的 ID。
    pub fn next() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        Self(NEXT.fetch_add(1, Ordering::Relaxed) + 1)
    }

    /// 读取 pane ID 的原始整数。
    ///
    /// 参数/输出：`self` 为输入；返回 ID 数值。
    pub fn as_u64(&self) -> u64 {
        self.0
    }
}

impl std::fmt::Display for PaneId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Display::fmt(&self.0, f)
    }
}

/// 后端类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaneKind {
    /// 本地 PTY/ConPTY 窗格。
    Local,
    /// russh SSH 窗格。
    Ssh,
    /// tmux 控制连接中的窗格。
    Tmux,
}

/// 新建 pane 的配置：按后端类型区分。
#[derive(Debug, Clone)]
pub enum PaneConfig {
    /// 启动本地 PTY 子进程的配置。
    Local {
        /// 要启动的程序路径或名称。
        command: String,
        /// 传给程序的命令行参数。
        args: Vec<String>,
        /// 子进程工作目录；`None` 表示继承当前目录。
        cwd: Option<String>,
        /// 初始终端列数。
        cols: u16,
        /// 初始终端行数。
        rows: u16,
    },
    /// 建立 russh SSH shell 的配置。
    Ssh {
        /// 远端主机名或 IP 地址。
        host: String,
        /// SSH 端口。
        port: u16,
        /// 远端登录用户名。
        user: String,
        /// 私钥路径；提供时优先于密码。
        identity: Option<String>,
        /// 登录密码；未提供私钥时使用。
        password: Option<String>,
        /// 初始终端列数。
        cols: u16,
        /// 初始终端行数。
        rows: u16,
    },
}

/// 单个逻辑窗格的统一异步句柄。
///
/// 实现必须保证每个实例只操作一个 pane，并隔离其输入、输出和生命周期。
#[async_trait]
pub trait PaneHandle: Send + Sync {
    /// 返回 pane 的进程内唯一标识。
    ///
    /// 参数/输出：`self` 为输入；返回 ID。
    fn id(&self) -> PaneId;

    /// 返回 pane 的后端类型。
    ///
    /// 参数/输出：`self` 为输入；返回类型枚举。
    fn kind(&self) -> PaneKind;

    /// 将句柄转为 `Any`，供上层按需 downcast 到具体实现。
    ///
    /// 参数/输出：`self` 为输入；返回动态类型引用。
    fn as_any(&self) -> &dyn std::any::Any;

    /// 向 pane 写入原始字节。
    ///
    /// 参数/输出：`self` 为输入；`data` 为输入字节；返回成功写入的字节数。
    async fn send_input(&self, data: &[u8]) -> Result<usize>;

    /// 调整远端或本地终端尺寸。
    ///
    /// 参数/输出：`self` 为输入；`size` 为输入尺寸；成功时无返回值。
    async fn resize(&self, size: Size) -> Result<()>;

    /// 订阅 pane 的后续输出字节。
    ///
    /// 参数/输出：`self` 为输入；返回广播接收端，订阅前的输出不会回放。
    fn subscribe(&self) -> broadcast::Receiver<Vec<u8>>;

    /// 关闭 pane 及其底层连接或子进程。
    ///
    /// 参数/输出：`self` 为输入；成功时无返回值。
    async fn close(&self) -> Result<()>;
}

/// 创建并管理一类 pane 的域接口。
///
/// 域负责后端差异、连接复用和 pane 工厂；上层只消费 [`PaneHandle`]。
#[async_trait]
pub trait Domain: Send + Sync {
    /// 返回该域创建的 pane 类型。
    ///
    /// 参数/输出：`self` 为输入；返回类型枚举。
    fn kind(&self) -> PaneKind;

    /// 按配置创建并连接一个 pane。
    ///
    /// 参数/输出：`self` 为输入；`config` 为输入配置；返回新 pane 的共享句柄。
    async fn spawn_pane(&self, config: &PaneConfig) -> Result<Arc<dyn PaneHandle>>;

    /// 关闭该域持有的公共资源。
    ///
    /// 参数/输出：`self` 为输入；成功时无返回值。各 pane 通常仍由自身句柄关闭。
    async fn close(&self) -> Result<()>;
}

#[cfg(test)]
mod tests {
    use super::PaneId;

    #[test]
    fn pane_id_display_respects_width() {
        assert_eq!(format!("{:<10}", PaneId::new(1)), "1         ");
        assert_eq!(format!("{:>10}", PaneId::new(1)), "         1");
    }
}
