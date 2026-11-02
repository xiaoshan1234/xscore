//! clap 命令定义：每段 argv 解析为一个 `Cmd`。
//!
//! `Cmd` 的 serde 派生就是 daemon 的 IPC 线协议，重构时不得重命名任何变体
//! 或字段，否则会与已在运行的 daemon 不兼容。

use clap::{Args, Parser, Subcommand, ValueEnum};
use serde::{Deserialize, Serialize};

#[derive(Debug, Parser)]
#[command(
    name = "xs",
    about = "terminal emulator CLI",
    disable_help_subcommand = true
)]
pub struct Command {
    #[command(subcommand)]
    pub command: Cmd,
}

#[derive(Debug, Clone, Copy, ValueEnum, Serialize, Deserialize)]
pub enum SplitDirection {
    #[value(name = "h")]
    Horizontal,
    #[value(name = "v")]
    Vertical,
}

#[derive(Debug, Clone, Subcommand, Serialize, Deserialize)]
pub enum Cmd {
    /// spawn a local shell pane
    Shell {
        program: String,
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// spawn a remote SSH shell pane
    Ssh {
        target: String,
        identity: Option<String>,
        /// resolved client-side; never parsed from argv
        #[arg(skip)]
        password: Option<String>,
    },
    /// spawn a WSL shell pane (Windows)
    Wsl {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// terminal conversation creation (shorthand: tml)
    #[command(subcommand)]
    Tml(TmlCmd),
    /// tmux session management (shorthand: tmx)
    #[command(subcommand)]
    Tmx(TmxCmd),
    /// attach to a tmux session
    Tmux(TmuxArgs),
    /// split the focused pane
    Split {
        direction: SplitDirection,
        program: String,
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// send text to a pane
    Write {
        pane: String,
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        text: Vec<String>,
    },
    /// print a pane's rendered screen
    Read { pane: String },
    /// resize a pane
    Resize { pane: String, cols: u16, rows: u16 },
    /// move focus
    Focus { pane: String },
    /// print the pane tree
    Layout,
    /// pane operations
    #[command(subcommand)]
    Pane(PaneCmd),
    /// window operations
    #[command(subcommand)]
    Window(WindowCmd),
    /// window group operations
    #[command(subcommand)]
    Group(GroupCmd),
    /// sync local panes with the tmux structure
    Sync,
    /// list panes
    List,
    /// print help
    Help,
    /// daemon lifecycle
    #[command(subcommand)]
    Daemon(DaemonCmd),
}

#[derive(Debug, Clone, Args, Serialize, Deserialize)]
pub struct TmuxArgs {
    pub session: String,
    /// tmux socket name (`tmux -L <NAME>`)
    #[arg(short = 'L', value_name = "NAME", conflicts_with = "socket_path")]
    pub socket_name: Option<String>,
    /// tmux socket path (`tmux -S <PATH>`)
    #[arg(short = 'S', value_name = "PATH", conflicts_with = "socket_name")]
    pub socket_path: Option<String>,
    /// run the tmux server on this SSH host
    #[arg(long, value_name = "DEST", conflicts_with = "wsl")]
    pub ssh: Option<String>,
    /// run the tmux server inside WSL
    #[arg(long, conflicts_with = "ssh")]
    pub wsl: bool,
    /// authenticate with a password
    #[arg(long, requires = "ssh")]
    pub password: bool,
    /// resolved client-side; never parsed from argv
    #[arg(skip)]
    pub password_value: Option<String>,
    /// extra ssh argument (repeatable)
    #[arg(
        long = "ssh-arg",
        value_name = "ARG",
        allow_hyphen_values = true,
        requires = "ssh"
    )]
    pub ssh_args: Vec<String>,
    /// extra wsl.exe argument (repeatable)
    #[arg(
        long = "wsl-arg",
        value_name = "ARG",
        allow_hyphen_values = true,
        requires = "wsl"
    )]
    pub wsl_args: Vec<String>,
}

#[derive(Debug, Clone, Subcommand, Serialize, Deserialize)]
pub enum TmlCmd {
    /// create a local shell/ssh/wsl conversation or a tmux window/pane
    Create(TmlCreateArgs),
}

#[derive(Debug, Clone, Args, Serialize, Deserialize)]
pub struct TmlCreateArgs {
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

#[derive(Debug, Clone, Subcommand, Serialize, Deserialize)]
pub enum TmxCmd {
    /// attach to a tmux session by URL
    Attach(TmxAttachArgs),
}

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

#[derive(Debug, Clone, Subcommand, Serialize, Deserialize)]
pub enum PaneCmd {
    /// delete a pane
    Del { pane: String },
    /// fetch tmux pane scrollback
    Fetch {
        pane: String,
        #[arg(long, value_name = "N")]
        line: Option<usize>,
    },
}

#[derive(Debug, Clone, Subcommand, Serialize, Deserialize)]
pub enum GroupCmd {
    /// list window groups
    List,
}

#[derive(Debug, Clone, Subcommand, Serialize, Deserialize)]
pub enum WindowCmd {
    /// create a new tmux window in a group
    Create { group: String, name: String },
    /// delete a tmux window in a group
    Del { group: String, window: String },
    /// list tmux windows (optionally scoped to a group)
    List { group: Option<String> },
    /// switch to a tmux window in a group
    Select { group: String, window: String },
}

#[derive(Debug, Clone, Subcommand, Serialize, Deserialize)]
pub enum DaemonCmd {
    /// run the daemon in the foreground
    Serve,
    /// print what the daemon currently holds
    Status,
    /// ask the daemon to exit
    Stop,
}
