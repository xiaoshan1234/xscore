//! window 命令：del / select / list（tmux window 维度的操作）。

use std::collections::HashMap;

use anyhow::{anyhow, Result};

use emulator::domain::PaneId;
use emulator::platform::tmux::quote_arg;

use crate::ui::app::App;
use crate::ui::render::Output;
use crate::ui::tmux::session::reconcile_session;
use crate::ui::window_group::{WindowId, WindowKind};

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
