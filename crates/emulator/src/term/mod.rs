//! L2 终端模拟核心：字节流 -> 屏幕网格。
//!
//! Milestone 1 只提供 `RawTerm`（原始字节历史 + 尺寸）；完整 VT 状态机在
//! Milestone 2 落地。

/// 每 pane 独立的终端核心。
pub trait TerminalCore: Send {
    fn feed(&mut self, bytes: &[u8]);
    fn snapshot(&self) -> String;
    fn resize(&mut self, cols: u16, rows: u16);
}

/// 原始字节核心：M1 的占位实现，仅累积字节并按 cap 截断。
pub struct RawTerm {
    buffer: Vec<u8>,
    cols: u16,
    rows: u16,
    cap: usize,
}

impl RawTerm {
    pub fn new(cols: u16, rows: u16) -> Self {
        Self {
            buffer: Vec::new(),
            cols,
            rows,
            cap: 1 << 20,
        }
    }

    pub fn cols(&self) -> u16 {
        self.cols
    }

    pub fn rows(&self) -> u16 {
        self.rows
    }
}

impl TerminalCore for RawTerm {
    fn feed(&mut self, bytes: &[u8]) {
        self.buffer.extend_from_slice(bytes);
        if self.buffer.len() > self.cap {
            let drop = self.buffer.len() - self.cap;
            self.buffer.drain(..drop);
        }
    }

    fn snapshot(&self) -> String {
        String::from_utf8_lossy(&self.buffer).into_owned()
    }

    fn resize(&mut self, cols: u16, rows: u16) {
        self.cols = cols;
        self.rows = rows;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn raw_term_accumulates_and_caps() {
        let mut term = RawTerm::new(80, 24);
        term.feed(b"hello ");
        term.feed(b"world");
        assert_eq!(term.snapshot(), "hello world");
    }

    #[test]
    fn raw_term_tracks_size() {
        let mut term = RawTerm::new(80, 24);
        term.resize(120, 40);
        assert_eq!((term.cols(), term.rows()), (120, 40));
    }
}
