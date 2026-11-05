use std::fs;
use std::path::Path;
use std::time::Duration;

use tokio::sync::broadcast;
use tokio::time::{timeout, Instant};

/// 集成测试从根目录 `test.config` 读取的连接参数。
#[derive(Clone)]
pub struct TestConfig {
    /// 完整 SSH 目标，格式为 `user@host`。
    pub ssh_destination: String,
    /// SSH 用户名。
    pub ssh_user: String,
    /// SSH 主机名或 IP。
    pub ssh_host: String,
    /// SSH TCP 端口。
    pub ssh_port: u16,
    /// SSH 登录密码。
    pub ssh_password: String,
}

impl TestConfig {
    /// 读取并校验工作区根目录的 `test.config`。
    ///
    /// 参数/输出：无输入；成功返回配置，文件或字段非法时 panic。
    pub fn load() -> Self {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../test.config");
        let contents = fs::read_to_string(&path).unwrap_or_else(|error| {
            panic!(
                "failed to read integration test config {}: {error}",
                path.display()
            )
        });

        Self::parse(&path.display().to_string(), &contents)
            .unwrap_or_else(|error| panic!("invalid integration test config: {error}"))
    }

    fn parse(source: &str, contents: &str) -> Result<Self, String> {
        let contents = contents.strip_prefix('\u{feff}').unwrap_or(contents);
        let mut ssh_destination = None;
        let mut ssh_password = None;
        let mut ssh_port = None;

        for (index, raw_line) in contents.lines().enumerate() {
            let line_number = index + 1;
            let line = raw_line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }

            let Some((key, value)) = line.split_once('=') else {
                return Err(format!("{source}:{line_number}: expected `key=value`"));
            };
            let key = key.trim();
            let value = value.trim();
            if value.is_empty() {
                return Err(format!(
                    "{source}:{line_number}: `{key}` must have a non-empty value"
                ));
            }

            let target = match key {
                "ssh_host" => &mut ssh_destination,
                "ssh_password" => &mut ssh_password,
                "ssh_port" => &mut ssh_port,
                _ => {
                    return Err(format!(
                        "{source}:{line_number}: unknown config key `{key}`"
                    ))
                }
            };
            if target.is_some() {
                return Err(format!(
                    "{source}:{line_number}: duplicate config key `{key}`"
                ));
            }
            *target = Some(value.to_string());
        }

        let ssh_destination =
            ssh_destination.ok_or_else(|| format!("{source}: missing required key `ssh_host`"))?;
        let ssh_password =
            ssh_password.ok_or_else(|| format!("{source}: missing required key `ssh_password`"))?;
        let ssh_port = ssh_port
            .as_deref()
            .unwrap_or("22")
            .parse::<u16>()
            .map_err(|_| format!("{source}: `ssh_port` must be a valid TCP port"))?;
        if ssh_port == 0 {
            return Err(format!("{source}: `ssh_port` must not be zero"));
        }

        let (ssh_user, ssh_host) = ssh_destination
            .split_once('@')
            .ok_or_else(|| format!("{source}: `ssh_host` must use `user@host` format"))?;
        if ssh_user.is_empty() || ssh_host.is_empty() {
            return Err(format!("{source}: `ssh_host` must use `user@host` format"));
        }
        let ssh_user = ssh_user.to_string();
        let ssh_host = ssh_host.to_string();

        Ok(Self {
            ssh_destination,
            ssh_user,
            ssh_host,
            ssh_port,
            ssh_password,
        })
    }
}

/// 从广播通道读取输出直到出现标记或超时。
///
/// 参数/输出：`receiver` 为输入输出；`needle` 和 `limit` 为输入；返回已读取字节。
pub async fn receive_until(
    receiver: &mut broadcast::Receiver<Vec<u8>>,
    needle: &[u8],
    limit: Duration,
) -> Vec<u8> {
    let deadline = Instant::now() + limit;
    let mut output = Vec::new();

    while Instant::now() < deadline {
        let remaining = deadline.saturating_duration_since(Instant::now());
        match timeout(remaining, receiver.recv()).await {
            Ok(Ok(chunk)) => {
                output.extend_from_slice(&chunk);
                if output.windows(needle.len()).any(|window| window == needle) {
                    break;
                }
            }
            Ok(Err(broadcast::error::RecvError::Lagged(_))) => continue,
            Ok(Err(broadcast::error::RecvError::Closed)) | Err(_) => break,
        }
    }

    output
}

#[cfg(test)]
mod tests {
    use super::TestConfig;

    #[test]
    fn config_parser_reads_values_without_echoing_password_errors() {
        let config = TestConfig::parse(
            "test.config",
            "\u{feff}# credentials\r\n ssh_host = user@host \r\nssh_password=p=a#ss\r\nssh_port=2222\r\n",
        )
        .unwrap();

        assert_eq!(config.ssh_destination, "user@host");
        assert_eq!(config.ssh_user, "user");
        assert_eq!(config.ssh_host, "host");
        assert_eq!(config.ssh_port, 2222);
        assert_eq!(config.ssh_password, "p=a#ss");

        let error = match TestConfig::parse("test.config", "ssh_password=secret-without-equals\n") {
            Ok(_) => panic!("expected malformed config line to fail"),
            Err(error) => error,
        };
        assert!(!error.contains("secret-without-equals"));
    }

    #[test]
    fn config_parser_reports_missing_required_key() {
        let error = match TestConfig::parse("test.config", "ssh_host=user@host\n") {
            Ok(_) => panic!("expected config without ssh_password to fail"),
            Err(error) => error,
        };

        assert!(error.contains("missing required key `ssh_password`"));
    }
}
