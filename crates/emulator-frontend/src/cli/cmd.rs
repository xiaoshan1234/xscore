//! clap 命令定义：每段 argv 解析为一个 `Cmd`。
//!
//! `Cmd` 的 serde 派生就是 daemon 的 IPC 线协议，重构时不得重命名任何变体
//! 或字段，否则会与已在运行的 daemon 不兼容。

use clap::{Args, Parser, Subcommand};
use serde::{Deserialize, Serialize};

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

/// 前端支持的全部命令。
#[derive(Debug, Clone, Subcommand, Serialize, Deserialize)]
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
    /// daemon lifecycle
    #[command(subcommand)]
    Daemon(DaemonCmd),
}

/// `tml` 子命令。
#[derive(Debug, Clone, Subcommand, Serialize, Deserialize)]
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
}

/// `tml add` 的参数。
#[derive(Debug, Clone, Args, Serialize, Deserialize)]
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
    /// resolved client-side; never parsed from argv
    #[arg(skip)]
    pub password: Option<String>,
}

/// `tmx` 子命令。
#[derive(Debug, Clone, Subcommand, Serialize, Deserialize)]
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
#[derive(Debug, Clone, Args, Serialize, Deserialize)]
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
#[derive(Debug, Clone, Subcommand, Serialize, Deserialize)]
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

/// daemon 生命周期子命令。
#[derive(Debug, Clone, Subcommand, Serialize, Deserialize)]
pub enum DaemonCmd {
    /// run the daemon in the foreground
    Serve,
    /// print what the daemon currently holds
    Status,
    /// ask the daemon to exit
    Stop,
}
