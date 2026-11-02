//! CLI 命令解析（clap derive）。REPL 每行输入解析为一个 `Cmd`。

use anyhow::{anyhow, Result};
use clap::{Args, Parser, Subcommand, ValueEnum};
use emulator::platform::tmux::{TmuxHostConfig, TmuxSocket};
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
    /// sync local panes with the tmux structure
    Sync,
    /// list panes
    List,
    /// print help
    Help,
    /// leave the REPL
    Quit,
    /// leave the REPL
    Exit,
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
pub enum WindowCmd {
    /// delete a tmux window
    Del { window: String },
    /// list tmux windows
    List,
    /// switch to another tmux window
    Select { window: String },
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

/// 解析一行 REPL 输入。
pub fn parse_line(line: &str) -> Result<Cmd, clap::Error> {
    parse_args(line.split_whitespace().map(str::to_string))
}

/// 解析一段命令行参数（argv，不含程序名）。
pub fn parse_args(args: impl IntoIterator<Item = String>) -> Result<Cmd, clap::Error> {
    Command::try_parse_from(std::iter::once("xs".to_string()).chain(args))
        .map(|command| command.command)
}

/// 解析 `user@host[:port]`。
pub fn parse_ssh_target(target: &str) -> Result<(String, String, u16)> {
    let (user, rest) = target
        .split_once('@')
        .ok_or_else(|| anyhow!("expected user@host[:port], got {target}"))?;
    let (host, port) = match rest.split_once(':') {
        Some((host, port)) => (
            host.to_string(),
            port.parse::<u16>().map_err(|_| anyhow!("invalid port"))?,
        ),
        None => (rest.to_string(), 22),
    };
    if user.is_empty() || host.is_empty() {
        return Err(anyhow!("invalid ssh target {target}"));
    }
    Ok((user.to_string(), host, port))
}

/// 把 clap 的 tmux 参数转换成 socket / host / 是否要密码。
pub fn build_tmux_config(args: &TmuxArgs) -> Result<(TmuxSocket, TmuxHostConfig, bool)> {
    let socket = match (&args.socket_name, &args.socket_path) {
        (Some(_), Some(_)) => return Err(anyhow!("-L and -S are mutually exclusive")),
        (Some(name), None) => TmuxSocket::Named(name.clone()),
        (None, Some(path)) => TmuxSocket::Path(path.clone()),
        (None, None) => TmuxSocket::Default,
    };

    let host = match (&args.ssh, args.wsl) {
        (Some(destination), false) => {
            if !args.wsl_args.is_empty() {
                return Err(anyhow!("--wsl-arg requires --wsl"));
            }
            TmuxHostConfig::Ssh {
                destination: destination.clone(),
                args: args.ssh_args.clone(),
                password: None,
                askpass: None,
            }
        }
        (None, true) => {
            if !args.ssh_args.is_empty() {
                return Err(anyhow!("--ssh-arg requires --ssh"));
            }
            TmuxHostConfig::Wsl {
                args: args.wsl_args.clone(),
            }
        }
        (Some(_), true) => return Err(anyhow!("--ssh and --wsl are mutually exclusive")),
        (None, false) => {
            if !args.ssh_args.is_empty() {
                return Err(anyhow!("--ssh-arg requires --ssh"));
            }
            if !args.wsl_args.is_empty() {
                return Err(anyhow!("--wsl-arg requires --wsl"));
            }
            TmuxHostConfig::Local
        }
    };

    Ok((socket, host, args.password))
}

/// 在边缘层解析交互式密码，避免 daemon 在无终端的进程里提示。
pub fn resolve_secrets(cmd: Cmd) -> Result<Cmd> {
    match cmd {
        Cmd::Ssh {
            target,
            identity,
            password: _,
        } => {
            let password = if identity.is_none() {
                Some(prompt_ssh_password(&target)?)
            } else {
                None
            };
            Ok(Cmd::Ssh {
                target,
                identity,
                password,
            })
        }
        Cmd::Tmux(mut tmux) => {
            if tmux.password {
                tmux.password_value = Some(prompt_tmux_password()?);
            }
            Ok(Cmd::Tmux(tmux))
        }
        other => Ok(other),
    }
}

fn prompt_ssh_password(target: &str) -> Result<String> {
    let (user, host, _) = parse_ssh_target(target)?;
    Ok(rpassword::prompt_password(format!(
        "Password for {user}@{host}: "
    ))?)
}

fn prompt_tmux_password() -> Result<String> {
    match std::env::var("XSCORE_SSH_PASSWORD").or_else(|_| std::env::var("PASSWD")) {
        Ok(password) => Ok(password),
        Err(_) => Ok(rpassword::prompt_password("Password: ")?),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ssh_target_with_default_port() {
        let (user, host, port) = parse_ssh_target("dev@box").unwrap();
        assert_eq!((user.as_str(), host.as_str(), port), ("dev", "box", 22));
    }

    #[test]
    fn ssh_target_with_port() {
        let (user, host, port) = parse_ssh_target("dev@box:2222").unwrap();
        assert_eq!((user.as_str(), host.as_str(), port), ("dev", "box", 2222));
    }

    #[test]
    fn ssh_target_requires_user() {
        assert!(parse_ssh_target("box").is_err());
    }

    fn parse(args: &[&str]) -> Result<Cmd, clap::Error> {
        parse_line(&args.join(" "))
    }

    #[test]
    fn tmux_default_local() {
        let Cmd::Tmux(tmux) = parse(&["tmux", "work", "-L", "x"]).unwrap() else {
            panic!("expected tmux command");
        };
        let (socket, host, want_password) = build_tmux_config(&tmux).unwrap();
        assert_eq!(socket, TmuxSocket::Named("x".into()));
        assert_eq!(host, TmuxHostConfig::Local);
        assert!(!want_password);
    }

    #[test]
    fn tmux_ssh_with_extra_args() {
        let Cmd::Tmux(tmux) = parse(&[
            "tmux",
            "work",
            "--ssh",
            "dev@box",
            "--ssh-arg",
            "-p",
            "--ssh-arg",
            "2222",
        ])
        .unwrap() else {
            panic!("expected tmux command");
        };
        let (_, host, _) = build_tmux_config(&tmux).unwrap();
        assert_eq!(
            host,
            TmuxHostConfig::Ssh {
                destination: "dev@box".into(),
                args: vec!["-p".into(), "2222".into()],
                password: None,
                askpass: None,
            }
        );
    }

    #[test]
    fn tmux_wsl() {
        let Cmd::Tmux(tmux) = parse(&[
            "tmux",
            "work",
            "--wsl",
            "--wsl-arg",
            "--distribution",
            "--wsl-arg",
            "Ubuntu",
        ])
        .unwrap() else {
            panic!("expected tmux command");
        };
        let (_, host, _) = build_tmux_config(&tmux).unwrap();
        assert_eq!(
            host,
            TmuxHostConfig::Wsl {
                args: vec!["--distribution".into(), "Ubuntu".into()],
            }
        );
    }

    #[test]
    fn tmux_ssh_and_wsl_conflict() {
        assert!(parse(&["tmux", "work", "--ssh", "dev@box", "--wsl"]).is_err());
    }

    #[test]
    fn tmux_password() {
        let Cmd::Tmux(tmux) = parse(&["tmux", "work", "--ssh", "dev@box", "--password"]).unwrap()
        else {
            panic!("expected tmux command");
        };
        let (_, host, want_password) = build_tmux_config(&tmux).unwrap();
        assert!(matches!(host, TmuxHostConfig::Ssh { .. }));
        assert!(want_password);
    }
}
