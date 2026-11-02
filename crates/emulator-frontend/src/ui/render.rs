//! L4 渲染（CLI 文本模拟）：把应用状态渲染为文本。
//!
//! 渲染层只读取快照与布局树，不持有 PaneHandle、不修改状态、不做 I/O。

use std::time::Duration;

use emulator::domain::{PaneId, PaneKind};
use emulator::term::{TerminalCore, VtTerm};
use tokio::sync::Mutex;

use crate::ui::app::App;

/// 追加式输出缓冲：`line` 等价于 `println!`（补一个换行），`push` 原样追加。
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

/// 把网格快照渲染为文本；先等待极短的静默窗口，让字节泵赶上。
pub async fn snapshot_text(term: &Mutex<VtTerm>) -> String {
    tokio::time::sleep(Duration::from_millis(60)).await;
    let mut text = term.lock().await.snapshot();
    if !text.ends_with('\n') {
        text.push('\n');
    }
    text
}

/// 渲染 pane 列表表格。
pub async fn pane_table(app: &App) -> Output {
    let mut out = Output::new();
    out.line(format!(
        "{:<6} {:<8} {:<12} {:<12} {:<5} {:<5}",
        "PANE", "KIND", "GROUP", "WINDOW", "COLS", "ROWS"
    ));
    let mut ids: Vec<PaneId> = app.panes.ids().collect();
    ids.sort_by_key(|p| p.as_u64());
    for id in ids {
        let entry = app.pane_entry(id).unwrap();
        let (cols, rows) = {
            let term = entry.term.lock().await;
            (term.cols(), term.rows())
        };
        let kind = match entry.handle.kind() {
            PaneKind::Local => "local",
            PaneKind::Ssh => "ssh",
            PaneKind::Tmux => "tmux",
        };
        let group = app
            .pane_group(id)
            .and_then(|gid| app.group_name(gid))
            .unwrap_or("-");
        let window = app
            .pane_window(id)
            .map(|window| window.to_string())
            .unwrap_or_else(|| "-".to_string());
        out.line(format!(
            "{:<6} {:<8} {:<12} {:<12} {:<5} {:<5}",
            id, kind, group, window, cols, rows
        ));
    }
    out
}

/// 渲染布局树文本。
pub fn layout_text(app: &App) -> Output {
    let mut out = Output::new();
    out.line(app.layout.render());
    if let Some(focused) = app.layout.focused() {
        out.line(format!("focused: {focused}"));
    }
    out
}

/// 渲染 daemon 状态摘要。
pub fn status_text(app: &App) -> Output {
    let mut out = Output::new();
    out.line(format!("daemon: {} pane(s)", app.panes.len()));
    let mut sessions: Vec<String> = app
        .tmux
        .values()
        .map(|m| m.session.clone())
        .collect::<std::collections::HashSet<_>>()
        .into_iter()
        .collect();
    sessions.sort();
    for session in sessions {
        out.line(format!("tmux session: {session}"));
    }
    out
}
