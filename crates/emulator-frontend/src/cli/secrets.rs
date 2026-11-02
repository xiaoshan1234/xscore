//! 在边缘层解析交互式密码，避免 daemon 在无终端的进程里提示。

use anyhow::Result;

use super::cmd::{Cmd, TmlCmd};
use super::parse::{parse_conversation_url, parse_ssh_target, ConversationSpec};

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
        Cmd::Tml(TmlCmd::Create(mut create)) => {
            if let ConversationSpec::Ssh {
                target,
                identity: None,
                password: None,
                ..
            } = parse_conversation_url(&create.url)?
            {
                create.password = Some(prompt_ssh_password(&target)?);
            }
            Ok(Cmd::Tml(TmlCmd::Create(create)))
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
