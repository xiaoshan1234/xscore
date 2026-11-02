//! window 命令：del / select / list（tmux window 维度的操作）。

use std::collections::HashMap;

use anyhow::{anyhow, Result};

use emulator::domain::PaneId;

use crate::ui::app::App;
use crate::ui::render::Output;
use crate::ui::tmux::session::reconcile_session;

pub async fn window_del(app: &mut App, window: String) -> Result<Output> {
    let pane_ids: Vec<PaneId> = app
        .tmux
        .iter()
        .filter(|(_, m)| m.window == window)
        .map(|(id, _)| *id)
        .collect();
    if pane_ids.is_empty() {
        return Err(anyhow!("tmux window not found: {window}"));
    }
    let session = app.tmux.get(&pane_ids[0]).unwrap().session.clone();
    let control = app
        .tmux_control(&session)
        .ok_or_else(|| anyhow!("tmux session control unavailable"))?;
    control.send(&format!("kill-window -t {window}")).await?;
    for id in pane_ids {
        app.remove_pane_local(id);
    }
    app.layout.remove_window(&window);
    let mut out = Output::new();
    out.line(format!("deleted window: {window}"));
    Ok(out)
}

pub async fn window_select(app: &mut App, handle: String) -> Result<Output> {
    let Some(session) = app
        .tmux
        .values()
        .find(|meta| meta.window == handle)
        .map(|meta| meta.session.clone())
    else {
        return Err(anyhow!("tmux window not found: {handle}"));
    };
    let control = app
        .tmux_control(&session)
        .ok_or_else(|| anyhow!("tmux session control unavailable"))?;
    control.send(&format!("select-window -t {handle}")).await?;
    let _ = reconcile_session(app, &session, &control).await?;
    let mut out = Output::new();
    out.line(format!("selected window: {handle}"));
    Ok(out)
}

pub fn window_list(app: &App) -> Output {
    let mut windows: HashMap<String, (bool, usize)> = HashMap::new();
    for meta in app.tmux.values() {
        let entry = windows
            .entry(meta.window.clone())
            .or_insert((meta.window_active, 0));
        entry.1 += 1;
    }
    let mut rows: Vec<_> = windows.into_iter().collect();
    rows.sort_by(|a, b| a.0.cmp(&b.0));
    let mut out = Output::new();
    out.line(format!("{:<10} {:<8} {:<8}", "WINDOW", "ACTIVE", "PANES"));
    for (handle, (active, pane_count)) in rows {
        out.line(format!(
            "{:<10} {:<8} {:<8}",
            handle,
            if active { "yes" } else { "no" },
            pane_count
        ));
    }
    out
}
