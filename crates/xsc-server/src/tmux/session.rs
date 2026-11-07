//! tmux 会话协调：attach 后的元数据、事件处理与结构 reconcile。

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use anyhow::Result;
use tokio::sync::Mutex;

use xscore::domain::{enumerate, make_terminal, TerminalId, TmuxTerminalInfo};
use xscore::platform::tmux::{TmuxControl, TmuxEvent};

use crate::app::App;
use crate::layout::{PaneRect, TmuxContainer};
use crate::tmux::events::{classify_tmux_event, diff_tmux_structure, StructureAction};

/// 本地 tmux pane 对应的元数据。
pub struct TmuxMeta {
    /// 所属 session 名。
    pub session: String,
    /// 所属 window 句柄。
    pub window: String,
    /// tmux pane 句柄。
    pub handle: String,
    /// window 是否为活动 window。
    pub window_active: bool,
    /// 本地几何矩形。
    pub rect: PaneRect,
    /// window 列数。
    pub window_cols: u16,
    /// window 行数。
    pub window_rows: u16,
}

/// 重新枚举并收敛一个 tmux session 的本地 pane 结构。
///
/// 参数/输出：`app` 为输入输出，会被就地更新；`session` 和 `control` 为输入；
/// 返回 `(删除数量, 新增数量)`。
pub async fn reconcile_session(
    app: &mut App,
    session: &str,
    control: &Arc<TmuxControl>,
) -> Result<(usize, usize)> {
    let windows = enumerate(control, session).await?;
    let local: HashMap<String, TerminalId> = app
        .tmux
        .iter()
        .filter(|(_, m)| m.session == session)
        .map(|(id, m)| (m.handle.clone(), *id))
        .collect();
    let (gone, new_panes) = diff_tmux_structure(&local, &windows);

    for id in &gone {
        app.remove_terminal_local(*id);
    }
    for spec in &new_panes {
        let pane_info = TmuxTerminalInfo {
            handle: spec.handle.clone(),
            cols: spec.cols,
            rows: spec.rows,
            x: spec.x,
            y: spec.y,
        };
        let new_pane = make_terminal(
            control.clone(),
            spec.window.clone(),
            &pane_info,
            spec.window_cols,
            spec.window_rows,
            spec.window_active,
        );
        let id = app.register_tmux_terminal(new_pane, spec.cols, spec.rows, session, &spec.window);
        app.tmux.insert(
            id,
            TmuxMeta {
                session: session.to_string(),
                window: spec.window.clone(),
                handle: spec.handle.clone(),
                window_active: spec.window_active,
                rect: PaneRect {
                    id,
                    x: spec.x,
                    y: spec.y,
                    cols: spec.cols,
                    rows: spec.rows,
                },
                window_cols: spec.window_cols,
                window_rows: spec.window_rows,
            },
        );
    }

    // 刷新已存在 pane 的几何与 active 标志（当前 window 可能切换）。
    for window in &windows {
        for pane in &window.panes {
            if let Some(id) = local.get(&pane.handle) {
                if let Some(meta) = app.tmux.get_mut(id) {
                    meta.window = window.handle.clone();
                    meta.window_active = window.active;
                    meta.rect = PaneRect {
                        id: *id,
                        x: pane.x,
                        y: pane.y,
                        cols: pane.cols,
                        rows: pane.rows,
                    };
                    meta.window_cols = window.cols;
                    meta.window_rows = window.rows;
                }
                app.update_tmux_window(*id, session, &window.handle);
            }
        }
    }

    rebuild_active_mirror(app, session);

    Ok((gone.len(), new_panes.len()))
}

/// 用当前 active tmux window 重建本地镜像布局。
///
/// 参数/输出：`app` 为输入输出；`session` 为输入；不返回值。
pub fn rebuild_active_mirror(app: &mut App, session: &str) {
    let windows: HashSet<String> = app
        .tmux
        .values()
        .filter(|meta| meta.session == session)
        .map(|meta| meta.window.clone())
        .collect();
    for window in &windows {
        app.layout.remove_window(window);
    }

    let Some(active_meta) = app
        .tmux
        .values()
        .find(|meta| meta.session == session && meta.window_active)
    else {
        return;
    };

    let mut rects: Vec<PaneRect> = app
        .tmux
        .values()
        .filter(|meta| meta.session == session && meta.window == active_meta.window)
        .map(|meta| meta.rect)
        .collect();
    rects.sort_by_key(|rect| (rect.y, rect.x));

    let active = rects.first().map(|rect| rect.id);
    let container = TmuxContainer {
        window: active_meta.window.clone(),
        cols: active_meta.window_cols,
        rows: active_meta.window_rows,
        panes: rects,
        active,
    };
    app.layout.add_mirror(container);
}

/// 将一个 tmux 事件应用到应用状态。
///
/// 参数/输出：`app` 为共享输入输出；`session`、`control`、`event` 为输入；
/// 返回是否应结束事件循环。
pub async fn handle_event(
    app: &Arc<Mutex<App>>,
    session: &str,
    control: &Arc<TmuxControl>,
    event: TmuxEvent,
) -> bool {
    match classify_tmux_event(&event) {
        StructureAction::RemoveWindow(window) => {
            let mut guard = app.lock().await;
            let pane_ids: Vec<TerminalId> = guard
                .tmux
                .iter()
                .filter(|(_, meta)| meta.session == session && meta.window == window)
                .map(|(id, _)| *id)
                .collect();
            for id in pane_ids {
                guard.remove_terminal_local(id);
            }
            false
        }
        StructureAction::Shutdown => {
            let mut guard = app.lock().await;
            let pane_ids: Vec<TerminalId> = guard
                .tmux
                .iter()
                .filter(|(_, meta)| meta.session == session)
                .map(|(id, _)| *id)
                .collect();
            for id in pane_ids {
                guard.remove_terminal_local(id);
            }
            true
        }
        StructureAction::ReconcileSession => {
            let mut guard = app.lock().await;
            if let Err(error) = reconcile_session(&mut guard, session, control).await {
                tracing::warn!(session, error = %error, "tmux event reconcile failed");
            }
            false
        }
    }
}
