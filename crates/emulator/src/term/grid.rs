//! 屏幕网格：单元格、光标、滚动与渲染快照。

/// 一个屏幕单元格。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cell {
    pub ch: char,
    /// 0 = 默认；非 0 为 ANSI 颜色索引（1..=16 的简化映射）。
    pub fg: u8,
    pub bg: u8,
    pub bold: bool,
    pub reverse: bool,
}

impl Default for Cell {
    fn default() -> Self {
        Self {
            ch: ' ',
            fg: 0,
            bg: 0,
            bold: false,
            reverse: false,
        }
    }
}

impl Cell {
    fn reset(&mut self) {
        *self = Cell::default();
    }
}

/// 0-based 光标位置。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Cursor {
    pub x: usize,
    pub y: usize,
}

/// 二维屏幕网格。
#[derive(Debug)]
pub struct Grid {
    cols: usize,
    rows: usize,
    cells: Vec<Vec<Cell>>,
    cursor: Cursor,
    saved: Cursor,
    fg: u8,
    bg: u8,
    bold: bool,
    reverse: bool,
}

impl Grid {
    pub fn new(cols: usize, rows: usize) -> Self {
        let cols = cols.max(1);
        let rows = rows.max(1);
        Self {
            cols,
            rows,
            cells: vec![vec![Cell::default(); cols]; rows],
            cursor: Cursor::default(),
            saved: Cursor::default(),
            fg: 0,
            bg: 0,
            bold: false,
            reverse: false,
        }
    }

    pub fn cols(&self) -> usize {
        self.cols
    }

    pub fn rows(&self) -> usize {
        self.rows
    }

    pub fn cursor_x(&self) -> usize {
        self.cursor.x
    }

    pub fn cursor_y(&self) -> usize {
        self.cursor.y
    }

    pub fn resize(&mut self, cols: usize, rows: usize) {
        let cols = cols.max(1);
        let rows = rows.max(1);
        let mut next = vec![vec![Cell::default(); cols]; rows];
        for (y, row) in next.iter_mut().enumerate().take(rows.min(self.rows)) {
            for (x, cell) in row.iter_mut().enumerate().take(cols.min(self.cols)) {
                *cell = self.cells[y][x];
            }
        }
        self.cols = cols;
        self.rows = rows;
        self.cells = next;
        self.cursor.x = self.cursor.x.min(cols.saturating_sub(1));
        self.cursor.y = self.cursor.y.min(rows.saturating_sub(1));
    }

    pub fn put_char(&mut self, ch: char) {
        let (x, y) = (self.cursor.x, self.cursor.y);
        let cell = &mut self.cells[y][x];
        cell.ch = ch;
        cell.fg = self.fg;
        cell.bg = self.bg;
        cell.bold = self.bold;
        cell.reverse = self.reverse;
        self.advance();
    }

    fn advance(&mut self) {
        if self.cursor.x + 1 < self.cols {
            self.cursor.x += 1;
        } else {
            self.cursor.x = 0;
            self.linefeed();
        }
    }

    pub fn linefeed(&mut self) {
        if self.cursor.y + 1 < self.rows {
            self.cursor.y += 1;
        } else {
            self.scroll_up(1);
        }
    }

    pub fn carriage_return(&mut self) {
        self.cursor.x = 0;
    }

    pub fn backspace(&mut self) {
        if self.cursor.x > 0 {
            self.cursor.x -= 1;
        }
    }

    pub fn tab(&mut self) {
        const TAB: usize = 8;
        self.cursor.x = ((self.cursor.x / TAB + 1) * TAB).min(self.cols.saturating_sub(1));
    }

    pub fn move_cursor(&mut self, x: usize, y: usize) {
        self.cursor.x = x.min(self.cols.saturating_sub(1));
        self.cursor.y = y.min(self.rows.saturating_sub(1));
    }

    pub fn cursor_up(&mut self, n: usize) {
        self.cursor.y = self.cursor.y.saturating_sub(n);
    }

    pub fn cursor_down(&mut self, n: usize) {
        self.cursor.y = (self.cursor.y + n).min(self.rows.saturating_sub(1));
    }

    pub fn cursor_left(&mut self, n: usize) {
        self.cursor.x = self.cursor.x.saturating_sub(n);
    }

    pub fn cursor_right(&mut self, n: usize) {
        self.cursor.x = (self.cursor.x + n).min(self.cols.saturating_sub(1));
    }

    pub fn erase_display(&mut self, mode: u32) {
        match mode {
            0 => {
                self.erase_line(0);
                for y in (self.cursor.y + 1)..self.rows {
                    self.clear_row(y);
                }
            }
            1 => {
                for y in 0..self.cursor.y {
                    self.clear_row(y);
                }
                for x in 0..=self.cursor.x {
                    self.cells[self.cursor.y][x].reset();
                }
            }
            _ => {
                for y in 0..self.rows {
                    self.clear_row(y);
                }
            }
        }
    }

    pub fn erase_line(&mut self, mode: u32) {
        match mode {
            0 => {
                for x in self.cursor.x..self.cols {
                    self.cells[self.cursor.y][x].reset();
                }
            }
            1 => {
                for x in 0..=self.cursor.x {
                    self.cells[self.cursor.y][x].reset();
                }
            }
            _ => self.clear_row(self.cursor.y),
        }
    }

    fn clear_row(&mut self, y: usize) {
        for cell in &mut self.cells[y] {
            cell.reset();
        }
    }

    fn scroll_up(&mut self, n: usize) {
        let n = n.min(self.rows);
        self.cells.drain(0..n);
        for _ in 0..n {
            self.cells.push(vec![Cell::default(); self.cols]);
        }
    }

    pub fn save_cursor(&mut self) {
        self.saved = self.cursor;
    }

    pub fn restore_cursor(&mut self) {
        self.cursor = self.saved;
    }

    pub fn clear(&mut self) {
        for row in &mut self.cells {
            for cell in row {
                cell.reset();
            }
        }
        self.cursor = Cursor::default();
        self.reset_attrs();
    }

    pub fn set_fg(&mut self, idx: u8) {
        self.fg = idx;
    }

    pub fn set_bg(&mut self, idx: u8) {
        self.bg = idx;
    }

    pub fn set_bold(&mut self, value: bool) {
        self.bold = value;
    }

    pub fn set_reverse(&mut self, value: bool) {
        self.reverse = value;
    }

    pub fn reset_attrs(&mut self) {
        self.fg = 0;
        self.bg = 0;
        self.bold = false;
        self.reverse = false;
    }

    /// 渲染为文本：每行去掉尾部空格，截掉末尾的空行。
    pub fn snapshot(&self) -> String {
        let mut last = 0usize;
        for (y, row) in self.cells.iter().enumerate() {
            if row.iter().any(|c| c.ch != ' ') {
                last = y;
            }
        }
        let mut out = String::new();
        for (y, row) in self.cells.iter().enumerate().take(last + 1) {
            let mut line: String = row.iter().map(|c| c.ch).collect();
            while line.ends_with(' ') {
                line.pop();
            }
            out.push_str(&line);
            if y < last {
                out.push('\n');
            }
        }
        out
    }
}
