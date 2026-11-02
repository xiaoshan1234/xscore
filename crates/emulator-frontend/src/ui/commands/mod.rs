//! 命令分发：把一条 `Cmd` 路由到对应的处理函数。

mod group;
mod local;
mod pane;
mod ssh;
mod tml;
mod tmux;
mod tmx;
mod window;

use std::sync::Arc;

use anyhow::{anyhow, Result};
use tokio::sync::Mutex;

use crate::cli::{Cmd, GroupCmd, PaneCmd, TmlCmd, TmxCmd, WindowCmd};
use crate::ui::app::App;
use crate::ui::render::{self, Output};

/// 分发一条命令，返回输出文本。
pub async fn dispatch(app: &Arc<Mutex<App>>, cmd: Cmd) -> Result<Output> {
    let mut guard = app.lock().await;
    let out = match cmd {
        Cmd::Shell { program, args } => local::shell(&mut guard, program, args).await?,
        Cmd::Ssh {
            target,
            identity,
            password,
        } => ssh::ssh(&mut guard, target, identity, password).await?,
        Cmd::Wsl { args } => local::wsl(&mut guard, args).await?,
        Cmd::Tml(tml_cmd) => match tml_cmd {
            TmlCmd::Create(create) => tml::create(&mut guard, create).await?,
        },
        Cmd::Tmx(tmx_cmd) => match tmx_cmd {
            TmxCmd::Attach(attach) => tmx::attach(&mut guard, &attach, app.clone()).await?,
        },
        Cmd::Tmux(tmux_args) => tmux::attach(&mut guard, &tmux_args, app.clone()).await?,
        Cmd::Split {
            direction,
            program,
            args,
        } => local::split(&mut guard, direction, program, args).await?,
        Cmd::Write { pane, text } => pane::write(&guard, pane, text).await?,
        Cmd::Read { pane } => pane::read(&guard, pane).await?,
        Cmd::Resize { pane, cols, rows } => pane::resize(&guard, pane, cols, rows).await?,
        Cmd::Focus { pane } => pane::focus(&mut guard, pane)?,
        Cmd::Layout => render::layout_text(&guard),
        Cmd::Pane(PaneCmd::Del { pane }) => pane::pane_del(&mut guard, pane).await?,
        Cmd::Pane(PaneCmd::Fetch { pane, line }) => pane::pane_fetch(&guard, pane, line).await?,
        Cmd::Window(WindowCmd::Create { group, name }) => {
            window::window_create(&mut guard, group, name).await?
        }
        Cmd::Window(WindowCmd::Del { group, window }) => {
            window::window_del(&mut guard, group, window).await?
        }
        Cmd::Window(WindowCmd::Select { group, window }) => {
            window::window_select(&mut guard, group, window).await?
        }
        Cmd::Window(WindowCmd::List { group }) => window::window_list(&guard, group)?,
        Cmd::Group(GroupCmd::List) => group::list(&guard),
        Cmd::Sync => tmux::sync(&mut guard).await?,
        Cmd::List => render::pane_table(&guard).await,
        Cmd::Help => help(),
        Cmd::Daemon(_) => return Err(anyhow!("daemon subcommand is handled by the entry point")),
    };
    Ok(out)
}

pub fn help() -> Output {
    let mut out = Output::new();
    out.line("commands:");
    out.line("  shell <program> [args...]   spawn a local shell pane");
    out.line("  ssh <user@host[:port]> [identity]   spawn a remote ssh pane");
    out.line("  wsl [args...]               spawn a WSL shell pane (Windows)");
    out.line("  tml create --url <url> [--new-window | --sph <id> | --spv <id>]   create a shell/ssh/wsl conversation");
    out.line("  tmux <session> [-L <socket>] [--ssh <dest> | --wsl] [--password]   attach to a tmux session");
    out.line(
        "  tmx attach --url <url> [--ssh <ssh-url> | --wsl <wsl-url>]   attach to a tmux session by URL",
    );
    out.line("  split h|v <program> [args...]   split the focused pane");
    out.line("  write <pane-id> <text>      send text + newline to the pane");
    out.line("  read  <pane-id>             print accumulated output");
    out.line("  resize <pane-id> <c> <r>    resize the pane");
    out.line("  focus <pane-id>             move focus");
    out.line("  layout                      print the pane tree");
    out.line("  pane del <pane-id>          delete a pane");
    out.line("  pane fetch <pane-id> [--line <N>]   fetch tmux pane scrollback (all, or last N)");
    out.line("  window create <group> <name>   create a tmux window in a group");
    out.line("  window del <group> <window>    delete a tmux window in a group");
    out.line("  window list [<group>]          list tmux windows (optionally scoped)");
    out.line("  window select <group> <window> switch to a tmux window in a group");
    out.line("  group list                     list window groups");
    out.line("  sync                        sync local panes with the tmux structure");
    out.line("  list                        list panes");
    out.line("  daemon serve|status|stop    run / inspect / stop the resident daemon");
    out
}
