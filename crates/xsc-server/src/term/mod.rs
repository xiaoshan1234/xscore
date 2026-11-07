//! L2 终端模拟核心：字节流 -> 屏幕网格。

pub mod grid;
pub mod vte;

pub use grid::Grid;
pub use vte::VtTerm;

/// 每个 pane 独占的终端状态机。
///
/// 核心只处理该 pane 的原始字节流，不感知来源是 Local、SSH 还是 tmux。
/// 不同 pane 之间不得共享实例，以免光标、字符集等 VT 状态相互串扰。
pub trait TerminalCore: Send {
    /// 增量消费终端原始字节；一个转义序列可以跨越多次调用。
    ///
    /// 参数/输出：`self` 为输入输出并更新网格状态；`bytes` 为输入；不返回值。
    fn feed(&mut self, bytes: &[u8]);

    /// 返回当前屏幕的文本快照，去掉每行末尾空格和末尾空行。
    ///
    /// 参数/输出：`self` 为输入；返回渲染后的文本。
    fn snapshot(&self) -> String;

    /// 调整网格尺寸，并保留仍位于新范围内的已有内容。
    ///
    /// 参数/输出：`self` 为输入输出；`cols` 和 `rows` 为输入；不返回值。
    fn resize(&mut self, cols: u16, rows: u16);
}
