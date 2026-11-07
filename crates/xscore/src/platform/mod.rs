//! L0 平台抽象：PTY/ConPTY、SSH、tmux 控制客户端。
//!
//! 该层不理解上层业务，只负责打开连接/进程并提供读/写/改尺寸。

pub mod pty;
pub mod ssh;
pub mod tmux;

pub use pty::LocalPty;

/// 窗格/终端尺寸（字符行列）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Size {
    /// 终端列数。
    pub cols: u16,
    /// 终端行数。
    pub rows: u16,
}

impl Size {
    /// 构造一个终端尺寸。
    ///
    /// 参数/输出：`cols` 和 `rows` 为输入；返回新的 [`Size`]。
    pub fn new(cols: u16, rows: u16) -> Self {
        Self { cols, rows }
    }
}
