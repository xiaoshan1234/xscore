//! xterm 子集状态机：把字节流解析成 Grid 操作。

use super::grid::Grid;
use super::TerminalCore;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    Ground,
    Escape,
    Csi,
    Osc,
    Charset,
}

/// 每 pane 独立的终端核心：Grid + 增量解析器。
pub struct VtTerm {
    grid: Grid,
    state: State,
    params: Vec<u32>,
    cur: String,
    osc: Vec<u8>,
    prev: u8,
}

impl VtTerm {
    pub fn new(cols: u16, rows: u16) -> Self {
        Self {
            grid: Grid::new(cols as usize, rows as usize),
            state: State::Ground,
            params: Vec::new(),
            cur: String::new(),
            osc: Vec::new(),
            prev: 0,
        }
    }

    pub fn cols(&self) -> usize {
        self.grid.cols()
    }

    pub fn rows(&self) -> usize {
        self.grid.rows()
    }

    fn feed_byte(&mut self, b: u8) {
        match self.state {
            State::Ground => self.ground(b),
            State::Escape => self.escape(b),
            State::Csi => self.csi(b),
            State::Osc => self.osc(b),
            State::Charset => self.state = State::Ground,
        }
    }

    fn ground(&mut self, b: u8) {
        match b {
            0x1B => self.state = State::Escape,
            0x07 => {}
            0x08 => self.grid.backspace(),
            0x09 => self.grid.tab(),
            0x0A..=0x0C => self.grid.linefeed(),
            0x0D => self.grid.carriage_return(),
            0x20..=0x7E => self.grid.put_char(b as char),
            0x00..=0x1A | 0x1C..=0x1F | 0x7F => {}
            _ => {}
        }
    }

    fn escape(&mut self, b: u8) {
        match b {
            b'[' => {
                self.state = State::Csi;
                self.params.clear();
                self.cur.clear();
            }
            b']' => {
                self.state = State::Osc;
                self.osc.clear();
                self.prev = 0;
            }
            b'(' | b')' | b'*' | b'+' => self.state = State::Charset,
            b'7' => {
                self.grid.save_cursor();
                self.state = State::Ground;
            }
            b'8' => {
                self.grid.restore_cursor();
                self.state = State::Ground;
            }
            b'c' => {
                self.grid.clear();
                self.state = State::Ground;
            }
            b'=' | b'>' | b'\\' => self.state = State::Ground,
            _ => self.state = State::Ground,
        }
    }

    fn csi(&mut self, b: u8) {
        match b {
            0x30..=0x3F => match b {
                b';' => self.push_param(),
                b'0'..=b'9' => self.cur.push(b as char),
                _ => {}
            },
            0x20..=0x2F => {}
            0x40..=0x7E => {
                self.push_param();
                let final_byte = b as char;
                let params = std::mem::take(&mut self.params);
                self.dispatch(final_byte, params);
                self.state = State::Ground;
            }
            _ => {}
        }
    }

    fn push_param(&mut self) {
        let value = if self.cur.is_empty() {
            0
        } else {
            self.cur.parse().unwrap_or(0)
        };
        self.params.push(value);
        self.cur.clear();
    }

    fn osc(&mut self, b: u8) {
        // OSC 以 BEL 或 ST（ESC \）结束；内容（标题等）不参与网格渲染。
        if self.prev == 0x1B && b == b'\\' {
            self.osc.clear();
            self.state = State::Ground;
            self.prev = 0;
            return;
        }
        if b == 0x07 {
            self.osc.clear();
            self.state = State::Ground;
            self.prev = 0;
            return;
        }
        self.osc.push(b);
        self.prev = b;
    }

    fn dispatch(&mut self, final_byte: char, params: Vec<u32>) {
        match final_byte {
            'H' | 'f' => {
                let row = self.param(&params, 0, 1).max(1) as usize;
                let col = self.param(&params, 1, 1).max(1) as usize;
                self.grid.move_cursor(col - 1, row - 1);
            }
            'A' => self
                .grid
                .cursor_up(self.param(&params, 0, 1).max(1) as usize),
            'B' => self
                .grid
                .cursor_down(self.param(&params, 0, 1).max(1) as usize),
            'C' => self
                .grid
                .cursor_right(self.param(&params, 0, 1).max(1) as usize),
            'D' => self
                .grid
                .cursor_left(self.param(&params, 0, 1).max(1) as usize),
            'G' => {
                let col = self.param(&params, 0, 1).max(1) as usize;
                self.grid.move_cursor(col - 1, self.grid.cursor_y());
            }
            'd' => {
                let row = self.param(&params, 0, 1).max(1) as usize;
                self.grid.move_cursor(self.grid.cursor_x(), row - 1);
            }
            'J' => self.grid.erase_display(self.param(&params, 0, 0)),
            'K' => self.grid.erase_line(self.param(&params, 0, 0)),
            'm' => self.sgr(&params),
            _ => {}
        }
    }

    fn param(&self, params: &[u32], index: usize, default: u32) -> u32 {
        params
            .get(index)
            .copied()
            .filter(|&v| v > 0)
            .unwrap_or(default)
    }

    fn sgr(&mut self, params: &[u32]) {
        if params.is_empty() || params.iter().all(|&p| p == 0) {
            self.grid.reset_attrs();
            return;
        }
        let mut i = 0;
        while i < params.len() {
            match params[i] {
                0 => self.grid.reset_attrs(),
                1 => self.grid.set_bold(true),
                7 => self.grid.set_reverse(true),
                22 => self.grid.set_bold(false),
                27 => self.grid.set_reverse(false),
                30..=37 => self.grid.set_fg((params[i] - 30) as u8 + 1),
                39 => self.grid.set_fg(0),
                40..=47 => self.grid.set_bg((params[i] - 40) as u8 + 1),
                49 => self.grid.set_bg(0),
                90..=97 => self.grid.set_fg((params[i] - 90) as u8 + 9),
                100..=107 => self.grid.set_bg((params[i] - 100) as u8 + 9),
                38 | 48 => break, // 扩展颜色未实现，停止解析后续 SGR
                _ => {}
            }
            i += 1;
        }
    }
}

impl TerminalCore for VtTerm {
    fn feed(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.feed_byte(b);
        }
    }

    fn snapshot(&self) -> String {
        self.grid.snapshot()
    }

    fn resize(&mut self, cols: u16, rows: u16) {
        self.grid.resize(cols as usize, rows as usize);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn feed(bytes: &[u8]) -> VtTerm {
        let mut term = VtTerm::new(80, 24);
        term.feed(bytes);
        term
    }

    #[test]
    fn plain_text_lands_on_grid() {
        assert_eq!(feed(b"hello").snapshot(), "hello");
    }

    #[test]
    fn newline_and_carriage_return() {
        assert_eq!(feed(b"ab\r\ncd").snapshot(), "ab\ncd");
    }

    #[test]
    fn cursor_positioning() {
        assert_eq!(feed(b"\x1b[2;3HX").snapshot(), "\n  X");
    }

    #[test]
    fn clear_screen() {
        assert_eq!(feed(b"abc\x1b[2J").snapshot(), "");
    }

    #[test]
    fn erase_line() {
        assert_eq!(feed(b"abc\x1b[2K").snapshot(), "");
    }

    #[test]
    fn sgr_does_not_change_text() {
        assert_eq!(feed(b"A\x1b[1mB\x1b[0mC").snapshot(), "ABC");
    }

    #[test]
    fn resize_preserves_content() {
        let mut term = feed(b"abc");
        term.resize(40, 12);
        assert_eq!(term.snapshot(), "abc");
        assert_eq!((term.cols(), term.rows()), (40, 12));
    }
}
