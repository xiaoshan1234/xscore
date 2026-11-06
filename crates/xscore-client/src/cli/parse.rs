//! 参数解析与边缘映射：argv -> `Cmd`。

use clap::Parser;
pub use xscore_protocol::{parse_conversation_url, parse_ssh_target, ConversationSpec};

use super::cmd::{Cmd, Command};

/// 解析一段命令行参数（argv，不含程序名）。
///
/// 参数/输出：`args` 为输入；成功返回 [`Cmd`]，解析失败返回 clap 错误。
pub fn parse_args(args: impl IntoIterator<Item = String>) -> Result<Cmd, clap::Error> {
    Command::try_parse_from(std::iter::once("xs".to_string()).chain(args))
        .map(|command| command.command)
}

/// 无参数时打印 clap 生成的帮助（替代已移除的交互式 REPL）。
///
/// 参数/输出：无输入；帮助写 stdout，I/O 失败时返回错误。
pub fn print_help() -> std::io::Result<()> {
    use clap::CommandFactory;

    let mut command = Command::command();
    command.print_help()?;
    println!();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::super::cmd::{LogFormatArg, LsCmd, TmlCmd, TmxCmd};
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
        assert!(add.sph.is_none());
        assert!(add.spv.is_none());
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
    fn tml_del_parses_pane() {
        let Cmd::Tml(TmlCmd::Del { pane }) = parse(&["tml", "del", "3"]).unwrap() else {
            panic!("expected tml command");
        };
        assert_eq!(pane, "3");
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

        let Cmd::Tml(TmlCmd::Log(args)) = parse(&[
            "tml",
            "log",
            "3",
            "--format",
            "raw",
            "--max-size",
            "2000",
            "--path",
            "logs/pane3.raw",
        ])
        .unwrap() else {
            panic!("expected tml log command");
        };
        assert_eq!(args.pane, "3");
        assert_eq!(args.format, LogFormatArg::Raw);
        assert_eq!(args.max_size, 2000);
        assert_eq!(
            args.path.as_deref(),
            Some(std::path::Path::new("logs/pane3.raw"))
        );
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
