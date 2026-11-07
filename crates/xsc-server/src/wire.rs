//! Conversion from clap command types to the daemon command model.

use anyhow::Result;

use crate::cli::{Cmd, LogFormatArg, LsCmd, TmlAddArgs, TmlCmd, TmlLogArgs, TmxAttachArgs, TmxCmd};
use crate::command::{
    CommandRequest, LogFormat, LsCommand, TmlAddRequest, TmlCommand, TmlLogRequest,
    TmxAttachRequest, TmxCommand,
};

/// Convert one locally parsed command into its dispatch form.
pub fn command_request(cmd: Cmd) -> Result<CommandRequest> {
    let command = match cmd {
        Cmd::Tml(cmd) => CommandRequest::Tml(tml_command(cmd)),
        Cmd::Tmx(cmd) => CommandRequest::Tmx(tmx_command(cmd)),
        Cmd::Layout { window } => CommandRequest::Layout { window },
        Cmd::Ls(cmd) => CommandRequest::Ls(ls_command(cmd)),
        Cmd::Help => CommandRequest::Help,
    };
    Ok(command)
}

fn tml_command(cmd: TmlCmd) -> TmlCommand {
    match cmd {
        TmlCmd::Add(args) => TmlCommand::Add(tml_add(args)),
        TmlCmd::Del { pane } => TmlCommand::Del { pane },
        TmlCmd::Write { pane, text } => TmlCommand::Write { pane, text },
        TmlCmd::Read { pane } => TmlCommand::Read { pane },
        TmlCmd::Resize { pane, cols, rows } => TmlCommand::Resize { pane, cols, rows },
        TmlCmd::Focus { pane } => TmlCommand::Focus { pane },
        TmlCmd::Fetch { pane, line } => TmlCommand::Fetch { pane, line },
        TmlCmd::Log(args) => TmlCommand::Log(tml_log(args)),
    }
}

fn tml_add(args: TmlAddArgs) -> TmlAddRequest {
    TmlAddRequest {
        url: args.url,
        group: args.group,
        new_window: args.new_window,
        sph: args.sph,
        spv: args.spv,
        password: args.password,
    }
}

fn tml_log(args: TmlLogArgs) -> TmlLogRequest {
    TmlLogRequest {
        pane: args.pane,
        format: match args.format {
            LogFormatArg::Raw => LogFormat::Raw,
            LogFormatArg::Char => LogFormat::Char,
        },
        max_size: args.max_size,
        path: args.path,
    }
}

fn tmx_command(cmd: TmxCmd) -> TmxCommand {
    match cmd {
        TmxCmd::Attach(args) => TmxCommand::Attach(tmx_attach(args)),
        TmxCmd::Sync { group_id } => TmxCommand::Sync { group_id },
    }
}

fn tmx_attach(args: TmxAttachArgs) -> TmxAttachRequest {
    TmxAttachRequest {
        url: args.url,
        ssh: args.ssh,
        wsl: args.wsl,
    }
}

fn ls_command(cmd: LsCmd) -> LsCommand {
    match cmd {
        LsCmd::Pane => LsCommand::Pane,
        LsCmd::Window { group } => LsCommand::Window { group },
        LsCmd::Group => LsCommand::Group,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::parse_args;

    #[test]
    fn tml_add_maps_all_transport_fields() {
        let cmd = parse_args(
            [
                "tml",
                "add",
                "--url",
                "shell://bash",
                "--group",
                "work",
                "--spv",
                "7",
            ]
            .into_iter()
            .map(str::to_string),
        )
        .expect("parse command");

        let CommandRequest::Tml(TmlCommand::Add(add)) =
            command_request(cmd).expect("convert command")
        else {
            panic!("expected tml add");
        };
        assert_eq!(add.url.as_deref(), Some("shell://bash"));
        assert_eq!(add.group.as_deref(), Some("work"));
        assert_eq!(add.spv.as_deref(), Some("7"));
        assert!(add.sph.is_none());
        assert!(add.new_window);
    }
}
