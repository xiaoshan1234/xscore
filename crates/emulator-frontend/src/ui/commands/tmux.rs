//! tmux 命令：attach / sync / status。

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use anyhow::{anyhow, Result};
use tokio::sync::Mutex;

use emulator::domain::TmuxDomain;
use emulator::platform::tmux::{TmuxHostConfig, TmuxSocket};

use crate::cli::{build_tmux_config, TmuxArgs};
use crate::ui::app::App;
use crate::ui::layout::{PaneRect, TmuxContainer};
use crate::ui::render::Output;
use crate::ui::tmux::session::{handle_event, reconcile_session, TmuxMeta};

pub async fn attach(
    app: &mut App,
    tmux_args: &TmuxArgs,
    app_arc: Arc<Mutex<App>>,
) -> Result<Output> {
    let session = tmux_args.session.clone();
    let (socket, mut host, want_password) = build_tmux_config(tmux_args)?;
    if want_password {
        let password = tmux_args
            .password_value
            .clone()
            .ok_or_else(|| anyhow!("password required for --ssh"))?;
        let askpass = std::env::current_exe()?.to_string_lossy().into_owned();
        if let TmuxHostConfig::Ssh {
            password: slot,
            askpass: ap,
            ..
        } = &mut host
        {
            *slot = Some(password);
            *ap = Some(askpass);
        }
    }
    attach_session(app, &host, &socket, &session, app_arc).await
}

pub(crate) async fn attach_session(
    app: &mut App,
    host: &TmuxHostConfig,
    socket: &TmuxSocket,
    session: &str,
    app_arc: Arc<Mutex<App>>,
) -> Result<Output> {
    let domain = TmuxDomain::new();
    let panes = domain.attach(host, socket, session).await?;
    let control = panes.first().map(|pane| pane.control().clone());

    let mut out = Output::new();
    let mut groups: Vec<(String, u16, u16, bool, Vec<PaneRect>)> = Vec::new();
    let mut index: HashMap<String, usize> = HashMap::new();
    for pane in panes {
        let (cols, rows) = (pane.cols(), pane.rows());
        let (x, y) = (pane.x(), pane.y());
        let window = pane.window().to_string();
        let (window_cols, window_rows) = (pane.window_cols(), pane.window_rows());
        let window_active = pane.window_active();
        let handle = pane.tmux_handle().to_string();
        let id = app.register_tmux_pane(pane, cols, rows, session);
        let rect = PaneRect {
            id,
            x,
            y,
            cols,
            rows,
        };
        app.tmux.insert(
            id,
            TmuxMeta {
                session: session.to_string(),
                window: window.clone(),
                handle,
                window_active,
                rect,
                window_cols,
                window_rows,
            },
        );
        let group = *index.entry(window.clone()).or_insert_with(|| {
            groups.push((window, window_cols, window_rows, window_active, Vec::new()));
            groups.len() - 1
        });
        groups[group].4.push(rect);
    }
    for (window, window_cols, window_rows, window_active, pane_rects) in groups {
        if window_active {
            let active = pane_rects.first().map(|rect| rect.id);
            let container = TmuxContainer {
                window: window.clone(),
                cols: window_cols,
                rows: window_rows,
                panes: pane_rects.clone(),
                active,
            };
            app.layout.add_mirror(container);
            out.line(format!(
                "tmux window {window} (active): {} pane(s)",
                pane_rects.len()
            ));
        } else {
            out.line(format!(
                "tmux window {window}: {} pane(s)",
                pane_rects.len()
            ));
        }
    }

    if let Some(control) = control {
        app.tmux_controls
            .insert(session.to_string(), control.clone());
        let mut events = control.subscribe();
        let event_app = app_arc.clone();
        let event_session = session.to_string();
        tokio::spawn(async move {
            while let Some(event) = events.recv().await {
                let should_stop = handle_event(&event_app, &event_session, &control, event).await;
                if should_stop {
                    break;
                }
            }
        });
    }
    Ok(out)
}

pub async fn sync(app: &mut App) -> Result<Output> {
    let sessions: Vec<String> = {
        let set: HashSet<String> = app.tmux.values().map(|m| m.session.clone()).collect();
        set.into_iter().collect()
    };
    let mut removed_total = 0usize;
    let mut added_total = 0usize;
    for session in sessions {
        let Some(control) = app.tmux_control(&session) else {
            continue;
        };
        let (removed, added) = reconcile_session(app, &session, &control).await?;
        removed_total += removed;
        added_total += added;
    }
    let mut out = Output::new();
    out.line(format!(
        "sync: removed {removed_total} pane(s), added {added_total} pane(s)"
    ));
    Ok(out)
}
