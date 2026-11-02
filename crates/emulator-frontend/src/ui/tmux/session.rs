//! tmux 会话协调：attach 后的元数据、事件处理与结构 reconcile。

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use anyhow::Result;
use tokio::sync::Mutex;

use emulator::domain::{enumerate, make_pane, PaneId, TmuxPaneInfo};
use emulator::platform::tmux::{TmuxControl, TmuxEvent};

use crate::ui::app::App;
use crate::ui::layout::{PaneRect, TmuxContainer};
use crate::ui::tmux::events::{classify_tmux_event, diff_tmux_structure, StructureAction};

pub struct TmuxMeta {
    pub session: String,
    pub window: String,
    pub handle: String,
    pub window_active: bool,
    pub rect: PaneRect,
    pub window_cols: u16,
    pub window_rows: u16,
}

pub async fn reconcile_session(
    app: &mut App,
    session: &str,
    control: &Arc<TmuxControl>,
) -> Result<(usize, usize)> {
    let windows = enumerate(control, session).await?;
    let local: HashMap<String, PaneId> = app
        .tmux
        .iter()
        .filter(|(_, m)| m.session == session)
        .map(|(id, m)| (m.handle.clone(), *id))
        .collect();
    let (gone, new_panes) = diff_tmux_structure(&local, &windows);

    for id in &gone {
        app.remove_pane_local(*id);
    }
    for spec in &new_panes {
        let pane_info = TmuxPaneInfo {
            handle: spec.handle.clone(),
            cols: spec.cols,
            rows: spec.rows,
            x: spec.x,
            y: spec.y,
        };
        let new_pane = make_pane(
            control.clone(),
            spec.window.clone(),
            &pane_info,
            spec.window_cols,
            spec.window_rows,
            spec.window_active,
        );
        let id = app.register_pane(new_pane, spec.cols, spec.rows);
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
            }
        }
    }

    rebuild_active_mirror(app, session);

    Ok((gone.len(), new_panes.len()))
}

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

pub async fn handle_event(
    app: &Arc<Mutex<App>>,
    session: &str,
    control: &Arc<TmuxControl>,
    event: TmuxEvent,
) -> bool {
    match classify_tmux_event(&event) {
        StructureAction::RemoveWindow(window) => {
            let mut guard = app.lock().await;
            let pane_ids: Vec<PaneId> = guard
                .tmux
                .iter()
                .filter(|(_, meta)| meta.session == session && meta.window == window)
                .map(|(id, _)| *id)
                .collect();
            for id in pane_ids {
                guard.remove_pane_local(id);
            }
            false
        }
        StructureAction::Shutdown => {
            let mut guard = app.lock().await;
            let pane_ids: Vec<PaneId> = guard
                .tmux
                .iter()
                .filter(|(_, meta)| meta.session == session)
                .map(|(id, _)| *id)
                .collect();
            for id in pane_ids {
                guard.remove_pane_local(id);
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
