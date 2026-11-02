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
    pub cols: u16,
    pub rows: u16,
}

impl Size {
    pub fn new(cols: u16, rows: u16) -> Self {
        Self { cols, rows }
    }
}
