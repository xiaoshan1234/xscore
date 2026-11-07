//! 命令分发：把一条协议命令路由到对应的处理函数。

mod group;
mod pane;
mod tml;
mod tmux;
mod tmx;
mod window;

use std::sync::Arc;

use anyhow::Result;
use tokio::sync::Mutex;
use xsc_protocol::{CommandRequest, LsCommand, TmlCommand, TmxCommand};

use crate::app::App;
use crate::render::{self, Output};

/// 分发一条命令，返回输出文本。
///
/// 参数/输出：`app` 为共享输入输出状态；`cmd` 为输入命令；返回命令输出。
pub async fn dispatch(app: &Arc<Mutex<App>>, cmd: CommandRequest) -> Result<Output> {
    let mut guard = app.lock().await;
    let out = match cmd {
        CommandRequest::Tml(tml_cmd) => match tml_cmd {
            TmlCommand::Add(add) => tml::add(&mut guard, add).await?,
            TmlCommand::Del { pane } => pane::pane_del(&mut guard, pane).await?,
            TmlCommand::Write { pane, text } => pane::write(&guard, pane, text).await?,
            TmlCommand::Read { pane } => pane::read(&guard, pane).await?,
            TmlCommand::Resize { pane, cols, rows } => {
                pane::resize(&guard, pane, cols, rows).await?
            }
            TmlCommand::Focus { pane } => pane::focus(&mut guard, pane)?,
            TmlCommand::Fetch { pane, line } => pane::pane_fetch(&guard, pane, line).await?,
            TmlCommand::Log(args) => pane::pane_log(&mut guard, args)?,
        },
        CommandRequest::Tmx(tmx_cmd) => match tmx_cmd {
            TmxCommand::Attach(attach) => tmx::attach(&mut guard, &attach, app.clone()).await?,
            TmxCommand::Sync { group_id } => tmux::sync(&mut guard, group_id).await?,
        },
        CommandRequest::Layout { window: None } => render::layout_text(&guard),
        CommandRequest::Layout { window: Some(spec) } => render::window_layout_text(&guard, &spec)?,
        CommandRequest::Ls(LsCommand::Pane) => render::pane_table(&guard).await,
        CommandRequest::Ls(LsCommand::Window { group }) => window::window_list(&guard, group)?,
        CommandRequest::Ls(LsCommand::Group) => group::list(&guard),
        CommandRequest::Help => help(),
    };
    Ok(out)
}

/// 构造基础命令帮助文本。
///
/// 参数/输出：无输入；返回可直接输出给用户的文本。
pub fn help() -> Output {
    let mut out = Output::new();
    out.line("commands:");
    out.line("  tml add [--url <url>] [--group <group>] [--new-window | --sph <id> | --spv <id>]   add a local or tmux conversation");
    out.line("  tml del <pane-id>          delete a pane");
    out.line("  tml write <pane-id> <text>  send text + newline to the pane");
    out.line("  tml read <pane-id>         print accumulated output");
    out.line("  tml resize <pane-id> <c> <r>   resize the pane");
    out.line("  tml focus <pane-id>        move focus");
    out.line("  tml fetch <pane-id> [--line <N>]   fetch tmux pane scrollback (all, or last N)");
    out.line("  tml log <pane-id> --format <raw|char> --max-size <N> [--path <file>]   record pane output");
    out.line(
        "  tmx attach --url <url> [--ssh <ssh-url> | --wsl <wsl-url>]   attach to a tmux session by URL",
    );
    out.line("  tmx sync <group-id>           sync one tmux group with its server");
    out.line("  layout [<window-id>]         print the pane tree or a window's layout");
    out.line("  ls pane|window [<group>]|group   list panes / windows / groups");
    out.line("  daemon serve|status|stop    run / inspect / stop the resident daemon");
    out
}
