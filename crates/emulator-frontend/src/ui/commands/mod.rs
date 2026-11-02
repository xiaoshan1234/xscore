//! 命令分发：把一条 `Cmd` 路由到对应的处理函数。

mod group;
mod pane;
mod tml;
mod tmux;
mod tmx;
mod window;

use std::sync::Arc;

use anyhow::{anyhow, Result};
use tokio::sync::Mutex;

use crate::cli::{Cmd, LsCmd, PaneCmd, TmlCmd, TmxCmd};
use crate::ui::app::App;
use crate::ui::render::{self, Output};

/// 分发一条命令，返回输出文本。
pub async fn dispatch(app: &Arc<Mutex<App>>, cmd: Cmd) -> Result<Output> {
    let mut guard = app.lock().await;
    let out = match cmd {
        Cmd::Tml(tml_cmd) => match tml_cmd {
            TmlCmd::Add(add) => tml::add(&mut guard, add).await?,
            TmlCmd::Del { pane } => pane::pane_del(&mut guard, pane).await?,
        },
        Cmd::Tmx(tmx_cmd) => match tmx_cmd {
            TmxCmd::Attach(attach) => tmx::attach(&mut guard, &attach, app.clone()).await?,
            TmxCmd::Sync { group_id } => tmux::sync(&mut guard, group_id).await?,
        },
        Cmd::Layout { window: None } => render::layout_text(&guard),
        Cmd::Layout { window: Some(spec) } => render::window_layout_text(&guard, &spec)?,
        Cmd::Pane(PaneCmd::Write { pane, text }) => pane::write(&guard, pane, text).await?,
        Cmd::Pane(PaneCmd::Read { pane }) => pane::read(&guard, pane).await?,
        Cmd::Pane(PaneCmd::Resize { pane, cols, rows }) => {
            pane::resize(&guard, pane, cols, rows).await?
        }
        Cmd::Pane(PaneCmd::Focus { pane }) => pane::focus(&mut guard, pane)?,
        Cmd::Pane(PaneCmd::Del { pane }) => pane::pane_del(&mut guard, pane).await?,
        Cmd::Pane(PaneCmd::Fetch { pane, line }) => pane::pane_fetch(&guard, pane, line).await?,
        Cmd::Ls(LsCmd::Pane) => render::pane_table(&guard).await,
        Cmd::Ls(LsCmd::Window { group }) => window::window_list(&guard, group)?,
        Cmd::Ls(LsCmd::Group) => group::list(&guard),
        Cmd::Help => help(),
        Cmd::Daemon(_) => return Err(anyhow!("daemon subcommand is handled by the entry point")),
    };
    Ok(out)
}

pub fn help() -> Output {
    let mut out = Output::new();
    out.line("commands:");
    out.line("  tml add [--url <url>] [--group <group>] [--new-window | --sph <id> | --spv <id>]   add a local or tmux conversation");
    out.line("  tml del <pane-id>          delete a pane");
    out.line(
        "  tmx attach --url <url> [--ssh <ssh-url> | --wsl <wsl-url>]   attach to a tmux session by URL",
    );
    out.line("  tmx sync <group-id>           sync one tmux group with its server");
    out.line("  layout [<window-id>]         print the pane tree or a window's layout");
    out.line("  pane write <pane-id> <text>  send text + newline to the pane");
    out.line("  pane read <pane-id>         print accumulated output");
    out.line("  pane resize <pane-id> <c> <r>   resize the pane");
    out.line("  pane focus <pane-id>        move focus");
    out.line("  pane del <pane-id>          delete a pane");
    out.line("  pane fetch <pane-id> [--line <N>]   fetch tmux pane scrollback (all, or last N)");
    out.line("  ls pane|window [<group>]|group   list panes / windows / groups");
    out.line("  daemon serve|status|stop    run / inspect / stop the resident daemon");
    out
}
