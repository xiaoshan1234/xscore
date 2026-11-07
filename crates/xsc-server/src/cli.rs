//! clap 命令面：把 `xs` 的 argv 解析为 `Cmd`。
//!
//! 命令解析由 daemon 完成，客户端只转发原始 argv。

use std::path::PathBuf;

use clap::{Args, Parser, Subcommand, ValueEnum};

/// `xs` 的顶层 clap 命令。
#[derive(Debug, Parser)]
#[command(
    name = "xs",
    about = "terminal emulator CLI",
    disable_help_subcommand = true
)]
pub struct Command {
    /// 实际子命令。
    #[command(subcommand)]
    pub command: Cmd,
}

/// daemon 支持的全部命令。
#[derive(Debug, Clone, Subcommand)]
pub enum Cmd {
    /// terminal pane/window management (shorthand: tml)
    #[command(subcommand)]
    Tml(TmlCmd),
    /// tmux session management (shorthand: tmx)
    #[command(subcommand)]
    Tmx(TmxCmd),
    /// print the pane tree, or a window's layout
    Layout {
        /// window id (Gxx-Tyy / Gxx-Myy); omit for the global tree
        window: Option<String>,
    },
    /// list resources (shorthand: ls)
    #[command(subcommand)]
    Ls(LsCmd),
    /// print help
    Help,
}

/// `tml` 子命令。
#[derive(Debug, Clone, Subcommand)]
pub enum TmlCmd {
    /// add a local shell/ssh/wsl conversation or a tmux window/pane
    Add(TmlAddArgs),
    /// delete a pane
    Del {
        /// pane id
        pane: String,
    },
    /// send text to a pane
    Write {
        /// 目标 pane ID。
        pane: String,
        /// 要拼接后发送的文本分段。
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        text: Vec<String>,
    },
    /// print a pane's rendered screen
    Read {
        /// 目标 pane ID。
        pane: String,
    },
    /// resize a pane
    Resize {
        /// 目标 pane ID。
        pane: String,
        /// 新列数。
        cols: u16,
        /// 新行数。
        rows: u16,
    },
    /// move focus
    Focus {
        /// 目标 pane ID。
        pane: String,
    },
    /// fetch tmux pane scrollback
    Fetch {
        /// 目标 pane ID。
        pane: String,
        /// 只取最近 N 行；省略时取全部历史。
        #[arg(long, value_name = "N")]
        line: Option<usize>,
    },
    /// record a pane's output to a local file
    Log(TmlLogArgs),
}

/// `tml add` 的参数。
#[derive(Debug, Clone, Args)]
pub struct TmlAddArgs {
    /// conversation URL (optional for tmux groups), e.g. shell://powershell?cwd=/home/me/src&cols=120&rows=40
    #[arg(long, value_name = "URL")]
    pub url: Option<String>,
    /// target window group (name or id); defaults to the default group
    #[arg(long, value_name = "GROUP")]
    pub group: Option<String>,
    /// create in a new window (default)
    #[arg(long, default_value_t = true, conflicts_with_all = ["sph", "spv"])]
    pub new_window: bool,
    /// split the specified pane horizontally to create
    #[arg(long, value_name = "ID", conflicts_with_all = ["new_window", "spv"])]
    pub sph: Option<String>,
    /// split the specified pane vertically to create
    #[arg(long, value_name = "ID", conflicts_with_all = ["new_window", "sph"])]
    pub spv: Option<String>,
    /// client-resolved password; hidden from help and only injected at the edge
    #[arg(long, hide = true)]
    pub password: Option<String>,
}

/// `tml log --format` 的格式参数。
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum LogFormatArg {
    /// 原样记录终端二进制字节。
    Raw,
    /// 过滤 ANSI/控制序列，按行写入带时间戳的文本。
    Char,
}

/// `tml log` 的参数。
#[derive(Debug, Clone, Args)]
pub struct TmlLogArgs {
    /// 目标 pane ID。
    pub pane: String,
    /// 日志格式。
    #[arg(long, value_enum)]
    pub format: LogFormatArg,
    /// 最大记录量；raw 按字节计，char 按行数计。
    #[arg(long, value_name = "N")]
    pub max_size: usize,
    /// 输出文件；省略时在当前目录生成带时间戳的文件。
    #[arg(long, value_name = "PATH")]
    pub path: Option<PathBuf>,
}

/// `tmx` 子命令。
#[derive(Debug, Clone, Subcommand)]
pub enum TmxCmd {
    /// attach to a tmux session by URL
    Attach(TmxAttachArgs),
    /// sync one tmux window group with its server-side structure
    Sync {
        /// window group id or group name
        group_id: String,
    },
}

/// `tmx attach` 的参数。
#[derive(Debug, Clone, Args)]
pub struct TmxAttachArgs {
    /// tmux target URL: tmux://<socket>/<session>
    #[arg(long, value_name = "URL")]
    pub url: String,
    /// run the tmux server on this SSH host: ssh://user@host[:port]?identity=<path>&password=<pwd>
    #[arg(long, value_name = "URL", conflicts_with = "wsl")]
    pub ssh: Option<String>,
    /// run the tmux server inside WSL: wsl://?args=--distribution,Ubuntu
    #[arg(long, value_name = "URL", conflicts_with = "ssh")]
    pub wsl: Option<String>,
}

/// `ls` 子命令。
#[derive(Debug, Clone, Subcommand)]
pub enum LsCmd {
    /// list panes
    Pane,
    /// list tmux windows (optionally scoped to a group)
    Window {
        /// 可选的 group 名或 ID。
        group: Option<String>,
    },
    /// list window groups
    Group,
}

/// 解析一段命令行参数（argv，不含程序名）。
///
/// 参数/输出：`args` 为输入；成功返回 [`Cmd`]，解析失败返回 clap 错误。
pub fn parse_args(args: impl IntoIterator<Item = String>) -> Result<Cmd, clap::Error> {
    Command::try_parse_from(std::iter::once("xs".to_string()).chain(args))
        .map(|command| command.command)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Result<Cmd, clap::Error> {
        parse_args(args.iter().map(|arg| (*arg).to_string()))
    }

    #[test]
    fn tml_new_window_is_default() {
        let Cmd::Tml(TmlCmd::Add(add)) = parse(&["tml", "add", "--url", "shell://bash"]).unwrap()
        else {
            panic!("expected tml command");
        };
        assert!(add.new_window);
        assert!(add.sph.is_none());
        assert!(add.spv.is_none());
    }

    #[test]
    fn tml_split_overrides_default_window() {
        let Cmd::Tml(TmlCmd::Add(add)) =
            parse(&["tml", "add", "--url", "shell://bash", "--sph", "3"]).unwrap()
        else {
            panic!("expected tml command");
        };
        assert!(add.new_window);
        assert_eq!(add.sph.as_deref(), Some("3"));
        assert!(add.spv.is_none());
    }

    #[test]
    fn tml_new_window_conflicts_with_split() {
        assert!(parse(&[
            "tml",
            "add",
            "--url",
            "shell://bash",
            "--new-window",
            "--sph",
            "3",
        ])
        .is_err());
    }

    #[test]
    fn tml_add_with_group() {
        let Cmd::Tml(TmlCmd::Add(add)) =
            parse(&["tml", "add", "--url", "shell://bash", "--group", "work"]).unwrap()
        else {
            panic!("expected tml command");
        };
        assert_eq!(add.group.as_deref(), Some("work"));
        assert!(add.new_window);
    }

    #[test]
    fn tml_add_tmux_group_without_url() {
        let Cmd::Tml(TmlCmd::Add(add)) =
            parse(&["tml", "add", "--group", "1", "--new-window"]).unwrap()
        else {
            panic!("expected tml command");
        };
        assert_eq!(add.group.as_deref(), Some("1"));
        assert!(add.url.is_none());
        assert!(add.new_window);
    }

    #[test]
    fn tml_add_accepts_injected_password() {
        let Cmd::Tml(TmlCmd::Add(add)) = parse(&[
            "tml",
            "add",
            "--url",
            "ssh://dev@box",
            "--password",
            "secret",
        ])
        .unwrap() else {
            panic!("expected tml command");
        };
        assert_eq!(add.password.as_deref(), Some("secret"));
    }

    #[test]
    fn ls_commands_parse() {
        assert!(matches!(
            parse(&["ls", "pane"]).unwrap(),
            Cmd::Ls(LsCmd::Pane)
        ));
        assert!(matches!(
            parse(&["ls", "window"]).unwrap(),
            Cmd::Ls(LsCmd::Window { group: None })
        ));
        assert!(matches!(
            parse(&["ls", "window", "work"]).unwrap(),
            Cmd::Ls(LsCmd::Window {
                group: Some(group)
            }) if group == "work"
        ));
        assert!(matches!(
            parse(&["ls", "group"]).unwrap(),
            Cmd::Ls(LsCmd::Group)
        ));
    }

    #[test]
    fn tmx_sync_parses_group_id() {
        assert!(matches!(
            parse(&["tmx", "sync", "2"]).unwrap(),
            Cmd::Tmx(TmxCmd::Sync { group_id }) if group_id == "2"
        ));
        assert!(parse(&["tmx", "sync"]).is_err());
    }

    #[test]
    fn tml_pane_operations_parse() {
        let Cmd::Tml(TmlCmd::Write { pane, text }) =
            parse(&["tml", "write", "3", "echo", "hello"]).unwrap()
        else {
            panic!("expected tml write command");
        };
        assert_eq!(pane, "3");
        assert_eq!(text, ["echo", "hello"]);

        assert!(matches!(
            parse(&["tml", "read", "3"]).unwrap(),
            Cmd::Tml(TmlCmd::Read { pane }) if pane == "3"
        ));
        assert!(matches!(
            parse(&["tml", "resize", "3", "120", "40"]).unwrap(),
            Cmd::Tml(TmlCmd::Resize {
                pane,
                cols: 120,
                rows: 40,
            }) if pane == "3"
        ));
        assert!(matches!(
            parse(&["tml", "focus", "3"]).unwrap(),
            Cmd::Tml(TmlCmd::Focus { pane }) if pane == "3"
        ));
        assert!(matches!(
            parse(&["tml", "fetch", "3", "--line", "50"]).unwrap(),
            Cmd::Tml(TmlCmd::Fetch {
                pane,
                line: Some(50),
            }) if pane == "3"
        ));
    }

    #[test]
    fn removed_commands_are_rejected() {
        for args in [
            &["split", "h", "bash"][..],
            &["split", "v", "bash"][..],
            &["write", "3", "echo", "hello"][..],
            &["read", "3"][..],
            &["resize", "3", "120", "40"][..],
            &["focus", "3"][..],
            &["pane", "write", "3", "echo", "hello"][..],
            &["pane", "read", "3"][..],
            &["pane", "resize", "3", "120", "40"][..],
            &["pane", "focus", "3"][..],
            &["pane", "del", "3"][..],
            &["pane", "fetch", "3"][..],
            &["sync"][..],
            &["list"][..],
            &["group", "list"][..],
            &["window", "create", "work", "shell"][..],
            &["window", "del", "work", "@1"][..],
            &["window", "list"][..],
            &["window", "select", "work", "@1"][..],
        ] {
            assert!(
                parse(args).is_err(),
                "expected removed command to fail: {}",
                args.join(" ")
            );
        }
    }

    #[test]
    fn layout_command_parses_window() {
        assert!(matches!(
            parse(&["layout"]).unwrap(),
            Cmd::Layout { window: None }
        ));
        assert!(matches!(
            parse(&["layout", "G2-T1"]).unwrap(),
            Cmd::Layout {
                window: Some(window)
            } if window == "G2-T1"
        ));
    }
}
