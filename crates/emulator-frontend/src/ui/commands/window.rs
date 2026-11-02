//! window 命令：del / select / list（tmux window 维度的操作）。

use std::collections::HashMap;

use anyhow::{anyhow, Result};

use emulator::domain::PaneId;
use emulator::platform::tmux::quote_arg;

use crate::ui::app::App;
use crate::ui::render::Output;
use crate::ui::tmux::session::reconcile_session;

pub async fn window_create(app: &mut App, group: String, name: String) -> Result<Output> {
    let gid = app
        .resolve_group(&group)
        .ok_or_else(|| anyhow!("group not found: {group}"))?;
    let session = app
        .group_session(gid)
        .ok_or_else(|| anyhow!("group {group} is not a tmux session group"))?
        .to_string();
    let control = app
        .tmux_control(&session)
        .ok_or_else(|| anyhow!("tmux session control unavailable"))?;
    control
        .send(&format!(
            "new-window -t {} -n {}",
            quote_arg(&session),
            quote_arg(&name)
        ))
        .await?;
    let _ = reconcile_session(app, &session, &control).await?;
    let mut out = Output::new();
    out.line(format!("created window: {name}"));
    Ok(out)
}

pub async fn window_del(app: &mut App, group: String, window: String) -> Result<Output> {
    let gid = app
        .resolve_group(&group)
        .ok_or_else(|| anyhow!("group not found: {group}"))?;
    let session = app
        .group_session(gid)
        .ok_or_else(|| anyhow!("group {group} is not a tmux session group"))?
        .to_string();
    let control = app
        .tmux_control(&session)
        .ok_or_else(|| anyhow!("tmux session control unavailable"))?;

    let pane_ids: Vec<PaneId> = app
        .tmux
        .iter()
        .filter(|(_, m)| m.session == session && m.window == window)
        .map(|(id, _)| *id)
        .collect();
    if pane_ids.is_empty() {
        return Err(anyhow!("tmux window not found: {window}"));
    }
    control
        .send(&format!("kill-window -t {}", quote_arg(&window)))
        .await?;
    for id in pane_ids {
        app.remove_pane_local(id);
    }
    app.layout.remove_window(&window);
    let mut out = Output::new();
    out.line(format!("deleted window: {window}"));
    Ok(out)
}

pub async fn window_select(app: &mut App, group: String, window: String) -> Result<Output> {
    let gid = app
        .resolve_group(&group)
        .ok_or_else(|| anyhow!("group not found: {group}"))?;
    let session = app
        .group_session(gid)
        .ok_or_else(|| anyhow!("group {group} is not a tmux session group"))?
        .to_string();
    let control = app
        .tmux_control(&session)
        .ok_or_else(|| anyhow!("tmux session control unavailable"))?;

    if !app
        .tmux
        .values()
        .any(|m| m.session == session && m.window == window)
    {
        return Err(anyhow!("tmux window not found: {window}"));
    }
    control
        .send(&format!("select-window -t {}", quote_arg(&window)))
        .await?;
    let _ = reconcile_session(app, &session, &control).await?;
    let mut out = Output::new();
    out.line(format!("selected window: {window}"));
    Ok(out)
}

pub fn window_list(app: &App, group: Option<String>) -> Result<Output> {
    let gid = match &group {
        Some(spec) => Some(
            app.resolve_group(spec)
                .ok_or_else(|| anyhow!("group not found: {spec}"))?,
        ),
        None => None,
    };

    let mut windows: HashMap<(String, String), (bool, usize)> = HashMap::new();
    for meta in app.tmux.values() {
        let session = meta.session.clone();
        if let Some(gid) = gid {
            if app.session_groups.get(&session) != Some(&gid) {
                continue;
            }
        }
        let entry = windows
            .entry((session, meta.window.clone()))
            .or_insert((meta.window_active, 0));
        entry.1 += 1;
    }

    let mut rows: Vec<_> = windows.into_iter().collect();
    rows.sort_by(|a, b| a.0.cmp(&b.0));

    let mut out = Output::new();
    out.line(format!(
        "{:<16} {:<10} {:<8} {:<6}",
        "SESSION", "WINDOW", "ACTIVE", "PANES"
    ));
    for ((session, window), (active, pane_count)) in rows {
        out.line(format!(
            "{:<16} {:<10} {:<8} {:<6}",
            session,
            window,
            if active { "yes" } else { "no" },
            pane_count
        ));
    }
    Ok(out)
}
