//! 命令输出缓冲：REPL、一次性 CLI、daemon 共用同一份文本表示。

/// 追加式输出文本。`line` 等价于 `println!`（补一个换行），`push` 原样追加。
#[derive(Debug, Default, Clone)]
pub struct Output {
    text: String,
}

impl Output {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn line(&mut self, value: impl AsRef<str>) {
        self.text.push_str(value.as_ref());
        self.text.push('\n');
    }

    pub fn push(&mut self, value: impl AsRef<str>) {
        self.text.push_str(value.as_ref());
    }

    pub fn into_string(self) -> String {
        self.text
    }
}
