//! `ls window` 的 tmux window 列表实现。

use std::collections::HashMap;

use anyhow::{anyhow, Result};

use crate::app::App;
use crate::render::Output;
use crate::window_group::{WindowId, WindowKind};

/// 列出全部或指定 group 的 window。
///
/// 参数/输出：`app` 为输入；`group` 为可选 group 名/ID 输入；返回表格文本。
pub fn window_list(app: &App, group: Option<String>) -> Result<Output> {
    let gid = match &group {
        Some(spec) => Some(
            app.resolve_group(spec)
                .ok_or_else(|| anyhow!("group not found: {spec}"))?,
        ),
        None => None,
    };

    let mut windows: HashMap<WindowId, (bool, usize)> = HashMap::new();
    for (pane, window) in &app.pane_windows {
        if let Some(gid) = gid {
            if window.group != gid {
                continue;
            }
        }
        let active = match window.kind {
            WindowKind::Tmux => app
                .tmux
                .get(pane)
                .map(|meta| meta.window_active)
                .unwrap_or(false),
            WindowKind::Manual => false,
        };
        let entry = windows.entry(*window).or_insert((active, 0));
        entry.1 += 1;
    }

    let mut rows: Vec<_> = windows.into_iter().collect();
    rows.sort_by_key(|(window, _)| {
        let kind = match window.kind {
            WindowKind::Tmux => 0u8,
            WindowKind::Manual => 1u8,
        };
        (window.group.as_u64(), kind, window.index)
    });

    let mut out = Output::new();
    out.line(format!(
        "{:<12} {:<16} {:<8} {:<6}",
        "WINDOW", "GROUP", "ACTIVE", "PANES"
    ));
    for (window, (active, pane_count)) in rows {
        let group_name = app.group_name(window.group).unwrap_or("-");
        let active_text = match window.kind {
            WindowKind::Tmux => {
                if active {
                    "yes"
                } else {
                    "no"
                }
            }
            WindowKind::Manual => "-",
        };
        out.line(format!(
            "{:<12} {:<16} {:<8} {:<6}",
            window, group_name, active_text, pane_count
        ));
    }
    Ok(out)
}
