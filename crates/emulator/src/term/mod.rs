//! L2 终端模拟核心：字节流 -> 屏幕网格。

pub mod grid;
pub mod vte;

pub use grid::Grid;
pub use vte::VtTerm;

/// 每 pane 独立的终端核心。
pub trait TerminalCore: Send {
    fn feed(&mut self, bytes: &[u8]);
    fn snapshot(&self) -> String;
    fn resize(&mut self, cols: u16, rows: u16);
}
