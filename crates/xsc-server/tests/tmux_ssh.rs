//! 真实环境 tmux 集成测试：SSH 主机上的 attach / split / kill-pane / 结构枚举。
//!
//! 配置从工作区根目录的 `test.config` 读取，格式见 `test-example.config`：
//!   ssh_host=user@host
//!   ssh_password=password
//!
//! 运行：
//!   cargo test -p xsc-server --test tmux_ssh -- --ignored --nocapture

use async_trait::async_trait;
use russh::client::{self, Config as SshConfig, Handler};
use russh::ChannelMsg;
use std::fs;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::time::timeout;
use xscore::domain::TmuxDomain;
use xscore::platform::tmux::{TmuxHostConfig, TmuxSocket};
use xscore::SshOptions;

static COUNTER: AtomicU64 = AtomicU64::new(0);

#[derive(Clone)]
struct TestConfig {
    ssh_host: String,
    ssh_password: String,
    ssh_port: u16,
}

impl TestConfig {
    fn load() -> Self {
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
        let mut ssh_host = None;
        let mut ssh_password = None;
        let mut ssh_port = None;

        for (index, raw_line) in contents.lines().enumerate() {
            let line_number = index + 1;
            let line = raw_line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }

            let Some((key, value)) = line.split_once('=') else {
                return Err(format!(
                    "{source}:{line_number}: expected `key=value`, got {line:?}"
                ));
            };
            let key = key.trim();
            let value = value.trim();
            if value.is_empty() {
                return Err(format!(
                    "{source}:{line_number}: `{key}` must have a non-empty value"
                ));
            }

            let target = match key {
                "ssh_host" => &mut ssh_host,
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

        Ok(Self {
            ssh_host: ssh_host
                .ok_or_else(|| format!("{source}: missing required key `ssh_host`"))?,
            ssh_password: ssh_password
                .ok_or_else(|| format!("{source}: missing required key `ssh_password`"))?,
            ssh_port: ssh_port
                .as_deref()
                .unwrap_or("22")
                .parse()
                .map_err(|_| format!("{source}: `ssh_port` must be a valid TCP port"))?,
        })
    }
}

#[derive(Clone, Default)]
struct TestSshHandler;

#[async_trait]
impl Handler for TestSshHandler {
    type Error = russh::Error;

    async fn check_server_key(
        &mut self,
        _server_public_key: &russh_keys::key::PublicKey,
    ) -> std::result::Result<bool, Self::Error> {
        Ok(true)
    }
}

async fn ssh_exec(config: &TestConfig, command: &str) -> Result<(), String> {
    let (user, host) = config
        .ssh_host
        .split_once('@')
        .ok_or_else(|| "ssh_host must use user@host format".to_string())?;
    timeout(Duration::from_secs(15), async {
        let mut handle = client::connect(
            Arc::new(SshConfig::default()),
            (host.to_string(), config.ssh_port),
            TestSshHandler,
        )
        .await
        .map_err(|error| format!("ssh connect: {error}"))?;
        let authenticated = handle
            .authenticate_password(user, config.ssh_password.clone())
            .await
            .map_err(|error| format!("ssh auth: {error}"))?;
        if !authenticated {
            return Err("ssh authentication failed".to_string());
        }

        let mut channel = handle
            .channel_open_session()
            .await
            .map_err(|error| format!("ssh channel: {error}"))?;
        channel
            .exec(true, command.as_bytes())
            .await
            .map_err(|error| format!("ssh exec: {error}"))?;

        let mut stderr = Vec::new();
        let mut exit_status = 0;
        loop {
            match channel.wait().await {
                Some(ChannelMsg::ExtendedData { data, .. }) => stderr.extend_from_slice(&data),
                Some(ChannelMsg::ExitStatus {
                    exit_status: status,
                }) => exit_status = status,
                Some(ChannelMsg::Eof | ChannelMsg::Close) | None => break,
                Some(_) => {}
            }
        }
        if exit_status != 0 {
            return Err(format!(
                "ssh command exited with {exit_status}: {}",
                String::from_utf8_lossy(&stderr)
            ));
        }
        Ok(())
    })
    .await
    .map_err(|_| "ssh command timed out".to_string())?
}

struct RemoteSession {
    config: TestConfig,
    name: String,
}

impl RemoteSession {
    async fn create(config: TestConfig) -> Self {
        let id = COUNTER.fetch_add(1, Ordering::Relaxed);
        let name = format!("xscore-it-{}-{id}", std::process::id());
        let session = Self { config, name };
        ssh_exec(
            &session.config,
            &format!("tmux new-session -d -s {} -x 100 -y 30", session.name),
        )
        .await
        .expect("create tmux session over russh");
        session
    }
}

impl Drop for RemoteSession {
    fn drop(&mut self) {
        let config = self.config.clone();
        let command = format!("tmux kill-session -t {}", self.name);
        let _ = std::thread::spawn(move || {
            let Ok(runtime) = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            else {
                return;
            };
            let _ = runtime.block_on(ssh_exec(&config, &command));
        })
        .join();
    }
}

#[tokio::test]
#[ignore = "requires test.config with a reachable SSH host"]
async fn ssh_tmux_attach_split_kill_sync() {
    let config = TestConfig::load();
    let session = RemoteSession::create(config.clone()).await;
    let (user, remote_host) = config
        .ssh_host
        .split_once('@')
        .expect("test.config ssh_host must be user@host");
    let host_config = TmuxHostConfig::Ssh {
        user: user.to_string(),
        host: remote_host.to_string(),
        port: config.ssh_port,
        identity: None,
        password: Some(config.ssh_password.clone()),
        options: SshOptions::default(),
    };
    let domain = TmuxDomain::new();
    let panes = domain
        .attach(&host_config, &TmuxSocket::Default, &session.name)
        .await
        .expect("attach to tmux session");

    assert!(!panes.is_empty(), "expected at least one pane");
    let window = panes[0].window().to_string();

    // split-window 后结构应变为 2 个 pane。
    let reply = panes[0]
        .send_command(&format!("split-window -d -t {window} -P -F '#{{pane_id}}'"))
        .await
        .expect("split-window");
    assert_eq!(reply.len(), 1, "split-window should print one pane id");
    let new_pane = reply[0].trim().to_string();
    let total = panes[0]
        .enumerate_windows()
        .await
        .expect("enumerate after split")
        .iter()
        .map(|w| w.panes.len())
        .sum::<usize>();
    assert_eq!(total, 2, "after split there should be 2 panes");

    // kill-pane 后结构应回落到 1 个 pane。
    panes[0]
        .send_command(&format!("kill-pane -t {new_pane}"))
        .await
        .expect("kill-pane");
    let total = panes[0]
        .enumerate_windows()
        .await
        .expect("enumerate after kill")
        .iter()
        .map(|w| w.panes.len())
        .sum::<usize>();
    assert_eq!(total, 1, "after kill there should be 1 pane");
}

#[cfg(test)]
mod tests {
    use super::TestConfig;

    #[test]
    fn config_parser_reads_literal_values() {
        let config = TestConfig::parse(
            "test.config",
            "\u{feff}# credentials\r\n ssh_host = user@host \r\nssh_password=p=a#ss\r\nssh_port=2222\r\n",
        )
        .unwrap();

        assert_eq!(config.ssh_host, "user@host");
        assert_eq!(config.ssh_password, "p=a#ss");
        assert_eq!(config.ssh_port, 2222);
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
