//! 在边缘层解析交互式密码，避免 daemon 在无终端的进程里提示。

use anyhow::Result;

use super::cmd::{Cmd, TmlCmd};
use super::parse::{parse_conversation_url, parse_ssh_target, ConversationSpec};

/// 在 CLI 边缘为缺少密钥和密码的 SSH 对话提示密码。
///
/// 参数/输出：`cmd` 为输入且可能被补充密码；返回可发送给 daemon 的命令。
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
