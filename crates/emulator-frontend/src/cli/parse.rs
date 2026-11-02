//! 参数解析与边缘映射：argv -> `Cmd`，以及 CLI 参数到后端类型的转换。

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
}
