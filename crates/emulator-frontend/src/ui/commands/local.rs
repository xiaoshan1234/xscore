//! 本地 pane 生命周期：shell / split / wsl。

use anyhow::Result;

use emulator::domain::{Domain, LocalDomain, PaneConfig, PaneId};

use crate::cli::SplitDirection;
use crate::ui::app::App;
use crate::ui::layout::Direction;
use crate::ui::render::Output;

pub async fn shell(app: &mut App, program: String, args: Vec<String>) -> Result<Output> {
    let id = spawn_local(app, program, args).await?;
    app.layout.add(id);
    let mut out = Output::new();
    out.line(format!("pane: {id}  kind: local  80x24"));
    Ok(out)
}

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

pub async fn wsl(app: &mut App, args: Vec<String>) -> Result<Output> {
    let id = spawn_local(app, "wsl.exe".to_string(), args).await?;
    app.layout.add(id);
    let mut out = Output::new();
    out.line(format!("pane: {id}  kind: wsl  80x24"));
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
    Ok(app.register_local_pane(handle, cols, rows))
}
