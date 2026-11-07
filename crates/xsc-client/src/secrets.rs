//! 边缘层交互式密码解析。
//!
//! daemon 以脱离终端的方式启动，无法自行提示密码，因此 `tml add --url
//! ssh://...` 的密码提示必须留在 client 侧：在把 argv 交给 daemon 之前补上
//! `--password`。其余命令解析全部由 daemon 完成。

use anyhow::Result;

/// 在缺少密钥和密码的 SSH 会话中提示密码，并把 `--password` 追加到 argv。
///
/// 参数/输出：`argv` 为输入输出，可能被追加密码参数；成功时无返回值。
pub fn resolve_secrets(argv: &mut Vec<String>) -> Result<()> {
    let Some(url) = find_url_value(argv) else {
        return Ok(());
    };
    if !needs_ssh_password(url) {
        return Ok(());
    }
    let target = ssh_target(url);
    let password = rpassword::prompt_password(format!("Password for {target}: "))?;
    argv.push("--password".to_string());
    argv.push(password);
    Ok(())
}

fn find_url_value(argv: &[String]) -> Option<&str> {
    let mut iter = argv.iter().map(String::as_str);
    while let Some(arg) = iter.next() {
        if arg == "--url" {
            return iter.next();
        }
        if let Some(value) = arg.strip_prefix("--url=") {
            return Some(value);
        }
    }
    None
}

fn needs_ssh_password(url: &str) -> bool {
    let Some(rest) = url.strip_prefix("ssh://") else {
        return false;
    };
    let query = rest.split('?').nth(1).unwrap_or("");
    !query.split('&').any(|pair| {
        let key = pair.split('=').next().unwrap_or(pair);
        key == "identity" || key == "password"
    })
}

fn ssh_target(url: &str) -> &str {
    let rest = url.strip_prefix("ssh://").unwrap_or(url);
    rest.split('?')
        .next()
        .unwrap_or(rest)
        .split('/')
        .next()
        .unwrap_or(rest)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn url_value_supports_both_forms() {
        let argv = vec![
            "tml".to_string(),
            "add".to_string(),
            "--url".to_string(),
            "ssh://dev@box".to_string(),
        ];
        assert_eq!(find_url_value(&argv), Some("ssh://dev@box"));

        let argv = vec![
            "tml".to_string(),
            "add".to_string(),
            "--url=ssh://dev@box".to_string(),
        ];
        assert_eq!(find_url_value(&argv), Some("ssh://dev@box"));
    }

    #[test]
    fn ssh_password_is_required_without_identity_or_password() {
        assert!(needs_ssh_password("ssh://dev@box"));
        assert!(needs_ssh_password("ssh://dev@box:2222?cols=80"));
        assert!(!needs_ssh_password(
            "ssh://dev@box?identity=~/.ssh/id_ed25519"
        ));
        assert!(!needs_ssh_password("ssh://dev@box?password=secret"));
        assert!(!needs_ssh_password("shell://bash"));
    }
}
