//! 参数解析与边缘映射：argv -> `Cmd`，以及 CLI 参数到后端类型的转换。

use std::collections::HashMap;

use anyhow::{anyhow, Result};
use clap::Parser;
use emulator::platform::tmux::{TmuxHostConfig, TmuxSocket};

use super::cmd::{Cmd, Command, TmuxArgs};

/// 解析一段命令行参数（argv，不含程序名）。
pub fn parse_args(args: impl IntoIterator<Item = String>) -> Result<Cmd, clap::Error> {
    Command::try_parse_from(std::iter::once("xs".to_string()).chain(args))
        .map(|command| command.command)
}

/// 无参数时打印 clap 生成的帮助（替代已移除的交互式 REPL）。
pub fn print_help() -> std::io::Result<()> {
    use clap::CommandFactory;

    let mut command = Command::command();
    command.print_help()?;
    println!();
    Ok(())
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

/// 把 clap 的 tmux 参数转换成 socket / host / 是否需要密码。
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

/// 对话创建规格：`tml create --url` 的解析结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConversationSpec {
    Shell {
        program: String,
        args: Vec<String>,
        cwd: Option<String>,
        cols: u16,
        rows: u16,
    },
    Ssh {
        target: String,
        identity: Option<String>,
        password: Option<String>,
        cols: u16,
        rows: u16,
    },
    Wsl {
        args: Vec<String>,
        cols: u16,
        rows: u16,
    },
}

impl ConversationSpec {
    pub fn scheme(&self) -> &'static str {
        match self {
            ConversationSpec::Shell { .. } => "shell",
            ConversationSpec::Ssh { .. } => "ssh",
            ConversationSpec::Wsl { .. } => "wsl",
        }
    }

    pub fn cols(&self) -> u16 {
        match self {
            ConversationSpec::Shell { cols, .. }
            | ConversationSpec::Ssh { cols, .. }
            | ConversationSpec::Wsl { cols, .. } => *cols,
        }
    }

    pub fn rows(&self) -> u16 {
        match self {
            ConversationSpec::Shell { rows, .. }
            | ConversationSpec::Ssh { rows, .. }
            | ConversationSpec::Wsl { rows, .. } => *rows,
        }
    }
}

/// 解析 `scheme://path?key=value&...` 形式的对话 URL。
pub fn parse_conversation_url(url: &str) -> Result<ConversationSpec> {
    let (scheme, rest) = url
        .split_once("://")
        .ok_or_else(|| anyhow!("expected scheme://... URL, got {url}"))?;
    let (path, query) = match rest.split_once('?') {
        Some((path, query)) => (path, query),
        None => (rest, ""),
    };
    let params = parse_query(query);

    match scheme {
        "shell" => {
            if path.is_empty() {
                return Err(anyhow!("shell URL requires a program: shell://<program>"));
            }
            let (cols, rows) = parse_size(&params)?;
            Ok(ConversationSpec::Shell {
                program: path.to_string(),
                args: parse_args_param(&params)?,
                cwd: params.get("cwd").cloned(),
                cols,
                rows,
            })
        }
        "ssh" => {
            if path.is_empty() {
                return Err(anyhow!("ssh URL requires user@host[:port]"));
            }
            let (cols, rows) = parse_size(&params)?;
            Ok(ConversationSpec::Ssh {
                target: path.to_string(),
                identity: params.get("identity").cloned(),
                password: params.get("password").cloned(),
                cols,
                rows,
            })
        }
        "wsl" => {
            let (cols, rows) = parse_size(&params)?;
            Ok(ConversationSpec::Wsl {
                args: parse_args_param(&params)?,
                cols,
                rows,
            })
        }
        other => Err(anyhow!("unsupported conversation scheme: {other}")),
    }
}

fn parse_query(query: &str) -> HashMap<String, String> {
    let mut params = HashMap::new();
    for pair in query.split('&').filter(|pair| !pair.is_empty()) {
        if let Some((key, value)) = pair.split_once('=') {
            params.insert(key.to_string(), value.to_string());
        }
    }
    params
}

fn parse_size(params: &HashMap<String, String>) -> Result<(u16, u16)> {
    let cols = match params.get("cols") {
        Some(value) => value
            .parse::<u16>()
            .map_err(|_| anyhow!("invalid cols: {value}"))?,
        None => 80,
    };
    let rows = match params.get("rows") {
        Some(value) => value
            .parse::<u16>()
            .map_err(|_| anyhow!("invalid rows: {value}"))?,
        None => 24,
    };
    Ok((cols, rows))
}

fn parse_args_param(params: &HashMap<String, String>) -> Result<Vec<String>> {
    match params.get("args") {
        Some(value) if value.is_empty() => Ok(Vec::new()),
        Some(value) => Ok(value.split(',').map(str::to_string).collect()),
        None => Ok(Vec::new()),
    }
}

#[cfg(test)]
mod tests {
    use super::super::cmd::TmlCmd;
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
        parse_args(args.iter().map(|arg| (*arg).to_string()))
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

    #[test]
    fn conversation_url_shell_defaults() {
        assert_eq!(
            parse_conversation_url("shell://powershell").unwrap(),
            ConversationSpec::Shell {
                program: "powershell".into(),
                args: vec![],
                cwd: None,
                cols: 80,
                rows: 24,
            }
        );
    }

    #[test]
    fn conversation_url_shell_with_query() {
        assert_eq!(
            parse_conversation_url("shell://powershell?cwd=/home/me/src&cols=120&rows=40").unwrap(),
            ConversationSpec::Shell {
                program: "powershell".into(),
                args: vec![],
                cwd: Some("/home/me/src".into()),
                cols: 120,
                rows: 40,
            }
        );
    }

    #[test]
    fn conversation_url_ssh() {
        assert_eq!(
            parse_conversation_url(
                "ssh://dev@box:2222?identity=~/.ssh/id_ed25519&cols=100&rows=30"
            )
            .unwrap(),
            ConversationSpec::Ssh {
                target: "dev@box:2222".into(),
                identity: Some("~/.ssh/id_ed25519".into()),
                password: None,
                cols: 100,
                rows: 30,
            }
        );
    }

    #[test]
    fn conversation_url_ssh_with_password() {
        assert_eq!(
            parse_conversation_url("ssh://dev@box?password=secret&cols=90&rows=28").unwrap(),
            ConversationSpec::Ssh {
                target: "dev@box".into(),
                identity: None,
                password: Some("secret".into()),
                cols: 90,
                rows: 28,
            }
        );
    }

    #[test]
    fn conversation_url_wsl_with_args() {
        assert_eq!(
            parse_conversation_url("wsl://?args=--distribution,Ubuntu&cols=90&rows=28").unwrap(),
            ConversationSpec::Wsl {
                args: vec!["--distribution".into(), "Ubuntu".into()],
                cols: 90,
                rows: 28,
            }
        );
    }

    #[test]
    fn conversation_url_requires_scheme() {
        assert!(parse_conversation_url("powershell").is_err());
    }

    #[test]
    fn tml_new_window_is_default() {
        let Cmd::Tml(TmlCmd::Create(create)) =
            parse(&["tml", "create", "--url", "shell://bash"]).unwrap()
        else {
            panic!("expected tml command");
        };
        assert!(create.new_window);
        assert!(create.sph.is_none());
        assert!(create.spv.is_none());
    }

    #[test]
    fn tml_split_overrides_default_window() {
        let Cmd::Tml(TmlCmd::Create(create)) =
            parse(&["tml", "create", "--url", "shell://bash", "--sph", "3"]).unwrap()
        else {
            panic!("expected tml command");
        };
        assert!(create.new_window);
        assert_eq!(create.sph.as_deref(), Some("3"));
        assert!(create.spv.is_none());
    }

    #[test]
    fn tml_new_window_conflicts_with_split() {
        assert!(parse(&[
            "tml",
            "create",
            "--url",
            "shell://bash",
            "--new-window",
            "--sph",
            "3",
        ])
        .is_err());
    }
}
