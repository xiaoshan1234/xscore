//! 在边缘层解析交互式密码，避免 daemon 在无终端的进程里提示。

use anyhow::Result;

use super::cmd::{Cmd, TmlCmd};
use super::parse::{parse_conversation_url, parse_ssh_target, ConversationSpec};

pub fn resolve_secrets(cmd: Cmd) -> Result<Cmd> {
    match cmd {
        Cmd::Tml(TmlCmd::Add(mut add)) => {
            if let Some(url) = &add.url {
                if let ConversationSpec::Ssh {
                    target,
                    identity: None,
                    password: None,
                    ..
                } = parse_conversation_url(url)?
                {
                    add.password = Some(prompt_ssh_password(&target)?);
                }
            }
            Ok(Cmd::Tml(TmlCmd::Add(add)))
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
