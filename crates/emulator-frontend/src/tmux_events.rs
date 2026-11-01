//! tmux 结构事件的纯决策逻辑：把事件映射为动作，并计算结构差集。

use emulator::domain::{PaneId, TmuxWindowInfo};
use emulator::platform::tmux::TmuxEvent;
use std::collections::HashMap;

/// 一个结构事件应触发的本地动作。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StructureAction {
    ReconcileSession,
    RemoveWindow(String),
    Shutdown,
}

/// 新增 pane 的描述：`(handle, window, cols, rows)`。
pub type PaneAddSpec = (String, String, u16, u16);

/// 把 tmux 结构事件映射为本地动作。
pub fn classify_tmux_event(event: &TmuxEvent) -> StructureAction {
    match event {
        TmuxEvent::WindowClose { handle } => StructureAction::RemoveWindow(handle.clone()),
        TmuxEvent::Exit { .. } => StructureAction::Shutdown,
        _ => StructureAction::ReconcileSession,
    }
}

/// 计算远端结构与本地句柄的差集。
///
/// 返回 `(需要删除的 PaneId, 需要新增的 (handle, window, cols, rows))`。
pub fn diff_tmux_structure(
    local: &HashMap<String, PaneId>,
    remote: &[TmuxWindowInfo],
) -> (Vec<PaneId>, Vec<PaneAddSpec>) {
    let mut desired: HashMap<String, (String, u16, u16)> = HashMap::new();
    for window in remote {
        for pane in &window.panes {
            desired.insert(
                pane.handle.clone(),
                (window.handle.clone(), pane.cols, pane.rows),
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
    for (handle, (window, cols, rows)) in &desired {
        if !local.contains_key(handle) {
            add_panes.push((handle.clone(), window.clone(), *cols, *rows));
        }
    }

    (remove_panes, add_panes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use emulator::domain::TmuxPaneInfo;

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
        local.insert("%0".to_string(), PaneId::new(10));
        local.insert("%1".to_string(), PaneId::new(11));

        let remote = vec![
            TmuxWindowInfo {
                handle: "@1".into(),
                name: "one".into(),
                panes: vec![TmuxPaneInfo {
                    handle: "%1".into(),
                    cols: 80,
                    rows: 24,
                }],
            },
            TmuxWindowInfo {
                handle: "@2".into(),
                name: "two".into(),
                panes: vec![TmuxPaneInfo {
                    handle: "%2".into(),
                    cols: 100,
                    rows: 30,
                }],
            },
        ];

        let (remove_panes, add_panes) = diff_tmux_structure(&local, &remote);
        assert_eq!(remove_panes, vec![PaneId::new(10)]);
        assert_eq!(
            add_panes,
            vec![("%2".to_string(), "@2".to_string(), 100, 30)]
        );
    }
}
