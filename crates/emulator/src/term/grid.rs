//! 屏幕网格：单元格、光标、滚动与渲染快照。

/// 一个屏幕单元格。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cell {
    /// 单元格显示的字符。
    pub ch: char,
    /// 0 = 默认；非 0 为 ANSI 颜色索引（1..=16 的简化映射）。
    pub fg: u8,
    /// 背景颜色索引；0 表示默认背景。
    pub bg: u8,
    /// 是否使用粗体属性。
    pub bold: bool,
    /// 是否交换前景色和背景色。
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
    /// 从 0 开始的列坐标。
    pub x: usize,
    /// 从 0 开始的行坐标。
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
    /// 创建至少为 1x1 的空网格。
    ///
    /// 参数/输出：`cols` 和 `rows` 为输入；返回初始化的 [`Grid`]。
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

    /// 返回网格列数。
    ///
    /// 参数/输出：`self` 为输入；返回列数。
    pub fn cols(&self) -> usize {
        self.cols
    }

    /// 返回网格行数。
    ///
    /// 参数/输出：`self` 为输入；返回行数。
    pub fn rows(&self) -> usize {
        self.rows
    }

    /// 返回光标列坐标。
    ///
    /// 参数/输出：`self` 为输入；返回从 0 开始的列坐标。
    pub fn cursor_x(&self) -> usize {
        self.cursor.x
    }

    /// 返回光标行坐标。
    ///
    /// 参数/输出：`self` 为输入；返回从 0 开始的行坐标。
    pub fn cursor_y(&self) -> usize {
        self.cursor.y
    }

    /// 调整网格尺寸并保留新范围内的已有内容。
    ///
    /// 参数/输出：`self` 为输入输出；`cols` 和 `rows` 为输入；不返回值。
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

    /// 在当前光标位置写入字符及当前属性，然后前进光标。
    ///
    /// 参数/输出：`self` 为输入输出；`ch` 为输入字符；不返回值。
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

    /// 将光标下移一行，到底部时向上滚动网格。
    ///
    /// 参数/输出：`self` 为输入输出；无其他参数；不返回值。
    pub fn linefeed(&mut self) {
        if self.cursor.y + 1 < self.rows {
            self.cursor.y += 1;
        } else {
            self.scroll_up(1);
        }
    }

    /// 将光标移动到当前行首。
    ///
    /// 参数/输出：`self` 为输入输出；无其他参数；不返回值。
    pub fn carriage_return(&mut self) {
        self.cursor.x = 0;
    }

    /// 将光标左移一列，已在行首时保持不动。
    ///
    /// 参数/输出：`self` 为输入输出；无其他参数；不返回值。
    pub fn backspace(&mut self) {
        if self.cursor.x > 0 {
            self.cursor.x -= 1;
        }
    }

    /// 将光标移动到下一个 8 列制表位。
    ///
    /// 参数/输出：`self` 为输入输出；无其他参数；不返回值。
    pub fn tab(&mut self) {
        const TAB: usize = 8;
        self.cursor.x = ((self.cursor.x / TAB + 1) * TAB).min(self.cols.saturating_sub(1));
    }

    /// 将光标移动到指定坐标，并限制在网格范围内。
    ///
    /// 参数/输出：`self` 为输入输出；`x` 和 `y` 为输入坐标；不返回值。
    pub fn move_cursor(&mut self, x: usize, y: usize) {
        self.cursor.x = x.min(self.cols.saturating_sub(1));
        self.cursor.y = y.min(self.rows.saturating_sub(1));
    }

    /// 将光标上移指定行数。
    ///
    /// 参数/输出：`self` 为输入输出；`n` 为输入行数；不返回值。
    pub fn cursor_up(&mut self, n: usize) {
        self.cursor.y = self.cursor.y.saturating_sub(n);
    }

    /// 将光标下移指定行数。
    ///
    /// 参数/输出：`self` 为输入输出；`n` 为输入行数；不返回值。
    pub fn cursor_down(&mut self, n: usize) {
        self.cursor.y = (self.cursor.y + n).min(self.rows.saturating_sub(1));
    }

    /// 将光标左移指定列数。
    ///
    /// 参数/输出：`self` 为输入输出；`n` 为输入列数；不返回值。
    pub fn cursor_left(&mut self, n: usize) {
        self.cursor.x = self.cursor.x.saturating_sub(n);
    }

    /// 将光标右移指定列数。
    ///
    /// 参数/输出：`self` 为输入输出；`n` 为输入列数；不返回值。
    pub fn cursor_right(&mut self, n: usize) {
        self.cursor.x = (self.cursor.x + n).min(self.cols.saturating_sub(1));
    }

    /// 按 VT 模式清除屏幕区域。
    ///
    /// 参数/输出：`self` 为输入输出；`mode` 为输入，0 表示光标至末尾，1 表示
    /// 开头至光标，其他值表示整个屏幕；不返回值。
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

    /// 按 VT 模式清除当前行区域。
    ///
    /// 参数/输出：`self` 为输入输出；`mode` 为输入，0 表示光标至行尾，1 表示
    /// 行首至光标，其他值表示整行；不返回值。
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

    /// 保存当前光标位置。
    ///
    /// 参数/输出：`self` 为输入输出；无其他参数；不返回值。
    pub fn save_cursor(&mut self) {
        self.saved = self.cursor;
    }

    /// 恢复最近保存的光标位置。
    ///
    /// 参数/输出：`self` 为输入输出；无其他参数；不返回值。
    pub fn restore_cursor(&mut self) {
        self.cursor = self.saved;
    }

    /// 清空全部单元格、光标位置和当前字符属性。
    ///
    /// 参数/输出：`self` 为输入输出；无其他参数；不返回值。
    pub fn clear(&mut self) {
        for row in &mut self.cells {
            for cell in row {
                cell.reset();
            }
        }
        self.cursor = Cursor::default();
        self.reset_attrs();
    }

    /// 设置后续写入使用的前景色索引。
    ///
    /// 参数/输出：`self` 为输入输出；`idx` 为输入颜色索引；不返回值。
    pub fn set_fg(&mut self, idx: u8) {
        self.fg = idx;
    }

    /// 设置后续写入使用的背景色索引。
    ///
    /// 参数/输出：`self` 为输入输出；`idx` 为输入颜色索引；不返回值。
    pub fn set_bg(&mut self, idx: u8) {
        self.bg = idx;
    }

    /// 设置后续写入是否使用粗体。
    ///
    /// 参数/输出：`self` 为输入输出；`value` 为输入布尔值；不返回值。
    pub fn set_bold(&mut self, value: bool) {
        self.bold = value;
    }

    /// 设置后续写入是否交换前景色和背景色。
    ///
    /// 参数/输出：`self` 为输入输出；`value` 为输入布尔值；不返回值。
    pub fn set_reverse(&mut self, value: bool) {
        self.reverse = value;
    }

    /// 将当前字符属性恢复为默认值。
    ///
    /// 参数/输出：`self` 为输入输出；无其他参数；不返回值。
    pub fn reset_attrs(&mut self) {
        self.fg = 0;
        self.bg = 0;
        self.bold = false;
        self.reverse = false;
    }

    /// 渲染为文本：每行去掉尾部空格，截掉末尾的空行。
    ///
    /// 参数/输出：`self` 为输入；返回文本快照。
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
