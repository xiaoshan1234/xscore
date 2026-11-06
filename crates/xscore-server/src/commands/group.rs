//! group 命令：列出 window group。

use crate::app::App;
use crate::render::Output;
use crate::window_group::{GroupKind, WindowGroup};

/// 列出所有 window group。
///
/// 参数/输出：`app` 为输入；返回 group 表格文本。
pub fn list(app: &App) -> Output {
    let mut out = Output::new();
    out.line(format!(
        "{:<6} {:<16} {:<8} {:<6}",
        "GROUP", "NAME", "KIND", "PANES"
    ));
    let mut groups: Vec<&WindowGroup> = app.groups.values().collect();
    groups.sort_by_key(|group| group.id.as_u64());
    for group in groups {
        let kind = match group.kind {
            GroupKind::Default => "default",
            GroupKind::Tmux => "tmux",
        };
        out.line(format!(
            "{:<6} {:<16} {:<8} {:<6}",
            group.id,
            group.name,
            kind,
            group.panes.len()
        ));
    }
    out
}
