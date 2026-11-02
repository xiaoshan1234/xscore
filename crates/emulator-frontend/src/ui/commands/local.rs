//! 本地 pane 生命周期：shell / split / wsl。

use anyhow::Result;

use emulator::domain::{Domain, LocalDomain, PaneConfig, PaneId};

use crate::cli::SplitDirection;
use crate::ui::app::App;
use crate::ui::layout::Direction;
use crate::ui::render::Output;
use crate::ui::window_group::WindowKind;

pub async fn split(
    app: &mut App,
    direction: SplitDirection,
    program: String,
    args: Vec<String>,
) -> Result<Output> {
    let direction = match direction {
        SplitDirection::Horizontal => Direction::Horizontal,
        SplitDirection::Vertical => Direction::Vertical,
    };
    let id = spawn_local(app, program, args).await?;
    app.layout.split_focused(direction, id);
    let mut out = Output::new();
    out.line(format!("pane: {id}  kind: local  80x24"));
    Ok(out)
}

async fn spawn_local(app: &mut App, command: String, args: Vec<String>) -> Result<PaneId> {
    let cols = 80u16;
    let rows = 24u16;
    let domain = LocalDomain::new();
    let handle = domain
        .spawn_pane(&PaneConfig::Local {
            command,
            args,
            cwd: None,
            cols,
            rows,
        })
        .await?;
    let window = app
        .layout
        .focused()
        .and_then(|focused| app.pane_window(focused))
        .filter(|window| window.kind == WindowKind::Manual);
    Ok(match window {
        Some(window) => app.register_pane_in_window(handle, cols, rows, window),
        None => app.register_pane_in_group(handle, cols, rows, app.default_group),
    })
}
