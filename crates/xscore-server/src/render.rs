//! L4 渲染（CLI 文本模拟）：把应用状态渲染为文本。
//!
//! 渲染层只读取快照与布局树，不持有 PaneHandle、不修改状态、不做 I/O。

use std::time::Duration;

use anyhow::Result;
use emulator::domain::{PaneId, PaneKind};
use emulator::term::{TerminalCore, VtTerm};
use tokio::sync::Mutex;

use crate::app::App;
use crate::layout::PaneRect;
use crate::window_group::{parse_window_id, WindowKind};

/// 追加式输出缓冲：`line` 等价于 `println!`（补一个换行），`push` 原样追加。
#[derive(Debug, Default, Clone)]
pub struct Output {
    text: String,
}

impl Output {
    /// 创建空文本输出。
    ///
    /// 参数/输出：无输入；返回空缓冲。
    pub fn new() -> Self {
        Self::default()
    }

    /// 追加一行文本并补换行。
    ///
    /// 参数/输出：`self` 为输入输出；`value` 为输入文本；不返回值。
    pub fn line(&mut self, value: impl AsRef<str>) {
        self.text.push_str(value.as_ref());
        self.text.push('\n');
    }

    /// 原样追加文本，不自动补换行。
    ///
    /// 参数/输出：`self` 为输入输出；`value` 为输入文本；不返回值。
    pub fn push(&mut self, value: impl AsRef<str>) {
        self.text.push_str(value.as_ref());
    }

    /// 取出缓冲中的完整文本。
    ///
    /// 参数/输出：`self` 为输入并消耗；返回文本字符串。
    pub fn into_string(self) -> String {
        self.text
    }
}

/// 把网格快照渲染为文本；先等待极短的静默窗口，让字节泵赶上。
///
/// 参数/输出：`term` 为输入；异步等待后返回以换行结尾的快照文本。
pub async fn snapshot_text(term: &Mutex<VtTerm>) -> String {
    tokio::time::sleep(Duration::from_millis(60)).await;
    let mut text = term.lock().await.snapshot();
    if !text.ends_with('\n') {
        text.push('\n');
    }
    text
}

/// 渲染 pane 列表表格。
///
/// 参数/输出：`app` 为输入；异步返回 pane 表格文本。
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

/// 渲染指定 window 的布局。
///
/// 参数/输出：`app` 和 `spec` 为输入；成功时返回文本，非法 window ID 返回错误。
pub fn window_layout_text(app: &App, spec: &str) -> Result<Output> {
    let window = parse_window_id(spec)?;
    let mut panes: Vec<PaneId> = app
        .pane_windows
        .iter()
        .filter(|(_, w)| **w == window)
        .map(|(pane, _)| *pane)
        .collect();
    panes.sort_by_key(|pane| pane.as_u64());

    let mut out = Output::new();
    match window.kind {
        WindowKind::Tmux => {
            let (cols, rows) = panes
                .iter()
                .find_map(|pane| app.tmux.get(pane))
                .map(|meta| (meta.window_cols, meta.window_rows))
                .unwrap_or((0, 0));
            out.line(format!("{window} ({}x{}):", cols, rows));
            let mut rects: Vec<PaneRect> = panes
                .iter()
                .filter_map(|pane| app.tmux.get(pane).map(|meta| meta.rect))
                .collect();
            rects.sort_by_key(|rect| (rect.y, rect.x));
            for rect in rects {
                out.line(format!(
                    "  P{}({},{} {}x{})",
                    rect.id, rect.x, rect.y, rect.cols, rect.rows
                ));
            }
        }
        WindowKind::Manual => {
            out.line(format!("{window}:"));
            for pane in panes {
                let kind = match app.pane_entry(pane).map(|entry| entry.handle.kind()) {
                    Some(PaneKind::Local) => "local",
                    Some(PaneKind::Ssh) => "ssh",
                    Some(PaneKind::Tmux) => "tmux",
                    None => "?",
                };
                out.line(format!("  P{pane} {kind}"));
            }
        }
    }
    Ok(out)
}

/// 渲染布局树文本。
///
/// 参数/输出：`app` 为输入；返回布局树和焦点文本。
pub fn layout_text(app: &App) -> Output {
    let mut out = Output::new();
    out.line(app.layout.render());
    if let Some(focused) = app.layout.focused() {
        out.line(format!("focused: {focused}"));
    }
    out
}

/// 渲染 daemon 状态摘要。
///
/// 参数/输出：`app` 为输入；返回 pane 与 tmux session 摘要。
pub fn status_text(app: &App) -> Output {
    let mut out = Output::new();
    out.line(format!(
        "daemon: {} pane(s), {} logger(s)",
        app.panes.len(),
        app.loggers.len()
    ));
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
