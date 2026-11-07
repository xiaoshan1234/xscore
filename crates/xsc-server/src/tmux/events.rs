//! tmux 结构事件的纯决策逻辑：把事件映射为动作，并计算结构差集。

use std::collections::HashMap;
use xscore::domain::{TerminalId, TmuxWindowInfo};
use xscore::platform::tmux::TmuxEvent;

/// 一个结构事件应触发的本地动作。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StructureAction {
    /// 重新枚举当前 session 的结构。
    ReconcileSession,
    /// 删除指定 tmux window 的本地镜像。
    RemoveWindow(String),
    /// 关闭会话并清理全部本地 pane。
    Shutdown,
}

/// 新增 pane 的完整描述：tmux 句柄、所属 window、几何信息。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaneAddSpec {
    /// tmux pane 句柄。
    pub handle: String,
    /// 所属 tmux window 句柄。
    pub window: String,
    /// pane 列数。
    pub cols: u16,
    /// pane 行数。
    pub rows: u16,
    /// pane 左偏移。
    pub x: u16,
    /// pane 上偏移。
    pub y: u16,
    /// window 列数。
    pub window_cols: u16,
    /// window 行数。
    pub window_rows: u16,
    /// window 是否为活动 window。
    pub window_active: bool,
}

/// 把 tmux 结构事件映射为本地动作。
///
/// 参数/输出：`event` 为输入；返回对应的 [`StructureAction`]。
pub fn classify_tmux_event(event: &TmuxEvent) -> StructureAction {
    match event {
        TmuxEvent::WindowClose { handle } => StructureAction::RemoveWindow(handle.clone()),
        TmuxEvent::Exit { .. } => StructureAction::Shutdown,
        _ => StructureAction::ReconcileSession,
    }
}

/// 计算远端结构与本地句柄的差集。
///
/// 返回 `(需要删除的 TerminalId, 需要新增的 PaneAddSpec)`。
/// 参数/输出：`local` 和 `remote` 为输入；返回待删除与待新增列表。
pub fn diff_tmux_structure(
    local: &HashMap<String, TerminalId>,
    remote: &[TmuxWindowInfo],
) -> (Vec<TerminalId>, Vec<PaneAddSpec>) {
    let mut desired: HashMap<String, PaneAddSpec> = HashMap::new();
    for window in remote {
        for pane in &window.panes {
            desired.insert(
                pane.handle.clone(),
                PaneAddSpec {
                    handle: pane.handle.clone(),
                    window: window.handle.clone(),
                    cols: pane.cols,
                    rows: pane.rows,
                    x: pane.x,
                    y: pane.y,
                    window_cols: window.cols,
                    window_rows: window.rows,
                    window_active: window.active,
                },
            );
        }
    }

    let mut remove_panes = Vec::new();
    for (handle, id) in local {
        if !desired.contains_key(handle) {
            remove_panes.push(*id);
        }
    }

    let mut add_panes = Vec::new();
    for (handle, spec) in &desired {
        if !local.contains_key(handle) {
            add_panes.push(spec.clone());
        }
    }

    (remove_panes, add_panes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use xscore::domain::TmuxTerminalInfo;

    fn pane_info(handle: &str, cols: u16, rows: u16, x: u16, y: u16) -> TmuxTerminalInfo {
        TmuxTerminalInfo {
            handle: handle.to_string(),
            cols,
            rows,
            x,
            y,
        }
    }

    #[test]
    fn classify_events() {
        assert_eq!(
            classify_tmux_event(&TmuxEvent::WindowClose {
                handle: "@1".into()
            }),
            StructureAction::RemoveWindow("@1".into())
        );
        assert_eq!(
            classify_tmux_event(&TmuxEvent::Exit {
                reason: String::new()
            }),
            StructureAction::Shutdown
        );
        assert_eq!(
            classify_tmux_event(&TmuxEvent::WindowAdd {
                handle: "@1".into()
            }),
            StructureAction::ReconcileSession
        );
    }

    #[test]
    fn diff_adds_and_removes() {
        let mut local = HashMap::new();
        local.insert("%0".to_string(), TerminalId::new(10));
        local.insert("%1".to_string(), TerminalId::new(11));

        let remote = vec![
            TmuxWindowInfo {
                handle: "@1".into(),
                name: "one".into(),
                cols: 80,
                rows: 24,
                active: false,
                panes: vec![pane_info("%1", 40, 24, 0, 0)],
            },
            TmuxWindowInfo {
                handle: "@2".into(),
                name: "two".into(),
                cols: 100,
                rows: 30,
                active: true,
                panes: vec![pane_info("%2", 50, 30, 0, 0)],
            },
        ];

        let (remove_panes, add_panes) = diff_tmux_structure(&local, &remote);
        assert_eq!(remove_panes, vec![TerminalId::new(10)]);
        assert_eq!(add_panes.len(), 1);
        assert_eq!(add_panes[0].handle, "%2");
        assert_eq!(add_panes[0].window, "@2");
        assert_eq!((add_panes[0].cols, add_panes[0].rows), (50, 30));
    }
}
