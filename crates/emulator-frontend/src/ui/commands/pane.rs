//! pane 命令：write / read / resize / focus / list / del / fetch。

use anyhow::{anyhow, Context, Result};

use emulator::domain::{PaneId, PaneKind};
use emulator::platform::Size;
use emulator::term::TerminalCore;

use crate::ui::app::App;
use crate::ui::render::{snapshot_text, Output};

pub async fn write(app: &App, pane: String, text: Vec<String>) -> Result<Output> {
    let id = parse_pane(app, &pane)?;
    let entry = app.pane_entry(id).unwrap();
    let mut data = text.join(" ").into_bytes();
    let eol: &[u8] = if entry.handle.kind() == PaneKind::Local && cfg!(windows) {
        b"\r\n"
    } else {
        b"\n"
    };
    data.extend_from_slice(eol);
    entry.handle.send_input(&data).await?;
    Ok(Output::new())
}

pub async fn read(app: &App, pane: String) -> Result<Output> {
    let id = parse_pane(app, &pane)?;
    let entry = app.pane_entry(id).unwrap();
    let mut out = Output::new();
    out.push(&snapshot_text(&entry.term).await);
    Ok(out)
}

pub async fn resize(app: &App, pane: String, cols: u16, rows: u16) -> Result<Output> {
    let id = parse_pane(app, &pane)?;
    let entry = app.pane_entry(id).unwrap();
    entry.handle.resize(Size::new(cols, rows)).await?;
    entry.term.lock().await.resize(cols, rows);
    Ok(Output::new())
}

pub fn focus(app: &mut App, pane: String) -> Result<Output> {
    let id = parse_pane(app, &pane)?;
    if let Some(window) = app.tmux.get(&id).map(|m| m.window.clone()) {
        if !app.layout.set_mirror_active(&window, id) {
            return Err(anyhow!("tmux window not found in layout: {window}"));
        }
    } else {
        app.layout.focus(id);
    }
    let mut out = Output::new();
    out.line(format!("focused: {id}"));
    Ok(out)
}

pub async fn pane_del(app: &mut App, pane: String) -> Result<Output> {
    let id = parse_pane(app, &pane)?;
    {
        let entry = app.pane_entry(id).unwrap();
        entry.handle.close().await?;
    }
    app.remove_pane_local(id);
    let mut out = Output::new();
    out.line(format!("deleted pane: {id}"));
    Ok(out)
}

pub async fn pane_fetch(app: &App, pane: String, line: Option<usize>) -> Result<Output> {
    let id = parse_pane(app, &pane)?;
    let start = match line {
        Some(line_count) => format!("-{line_count}"),
        None => "-".to_string(),
    };
    let meta = app
        .tmux
        .get(&id)
        .ok_or_else(|| anyhow!("pane {id} is not a tmux pane"))?;
    let session = &meta.session;
    let control = app
        .tmux_control(session)
        .ok_or_else(|| anyhow!("tmux session control unavailable"))?;
    let handle = &meta.handle;
    let lines = control
        .send(&format!("capture-pane -p -S {start} -t {handle}"))
        .await?;
    let mut out = Output::new();
    for line in &lines {
        out.line(line.trim_end());
    }
    Ok(out)
}

fn parse_pane(app: &App, s: &str) -> Result<PaneId> {
    let id = PaneId::new(s.parse().context("pane id")?);
    if !app.panes.contains(id) {
        return Err(anyhow!("pane not found: {id}"));
    }
    Ok(id)
}
