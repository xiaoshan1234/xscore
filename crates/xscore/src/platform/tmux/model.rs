//! tmux 目标模型：宿主机与 socket 选择。

/// tmux 控制连接所在的宿主机类型。
#[derive(Clone, Default, PartialEq)]
pub enum TmuxHostConfig {
    /// 直接在当前主机启动 tmux。
    #[default]
    Local,
    /// 通过 russh 连接远端主机并执行 tmux 控制客户端。
    Ssh {
        /// 远端登录用户名。
        user: String,
        /// 远端主机名或 IP 地址。
        host: String,
        /// SSH 端口。
        port: u16,
        /// 可选的私钥路径；提供时优先于密码。
        identity: Option<String>,
        /// 可选的登录密码。
        password: Option<String>,
        /// Optional connection-level knobs for the SSH transport.
        options: crate::platform::ssh::SshOptions,
    },
    /// 通过 `wsl.exe` 在 WSL 中启动 tmux。
    Wsl {
        /// 追加给 `wsl.exe` 的参数。
        args: Vec<String>,
    },
}

impl std::fmt::Debug for TmuxHostConfig {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Local => formatter.write_str("Local"),
            Self::Ssh {
                user,
                host,
                port,
                identity,
                password,
                options,
            } => formatter
                .debug_struct("Ssh")
                .field("user", user)
                .field("host", host)
                .field("port", port)
                .field("identity", identity)
                .field("password", &password.as_ref().map(|_| "<redacted>"))
                .field("options", options)
                .finish(),
            Self::Wsl { args } => formatter.debug_struct("Wsl").field("args", args).finish(),
        }
    }
}

/// tmux server 的 socket 选择。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TmuxSocket {
    /// 使用 tmux 默认 socket。
    Default,
    /// 使用 `tmux -L` 指定的命名 socket。
    Named(String),
    /// 使用 `tmux -S` 指定的 socket 路径。
    Path(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ssh_debug_output_redacts_password() {
        let host = TmuxHostConfig::Ssh {
            user: "dev".into(),
            host: "example.com".into(),
            port: 22,
            identity: None,
            password: Some("do-not-log".into()),
            options: crate::platform::ssh::SshOptions::default(),
        };

        let rendered = format!("{host:?}");
        assert!(rendered.contains("password: Some(\"<redacted>\")"));
        assert!(!rendered.contains("do-not-log"));
    }
}
