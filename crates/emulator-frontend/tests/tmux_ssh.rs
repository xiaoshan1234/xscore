//! 真实环境 tmux 集成测试：SSH 主机上的 attach / split / kill-pane / 结构枚举。
//!
//! 配置从工作区根目录的 `test.config` 读取，格式见 `test-example.config`：
//!   ssh_host=user@host
//!   ssh_password=password
//!
//! 运行：
//!   cargo test -p emulator-frontend --test tmux_ssh -- --ignored --nocapture

use emulator::domain::TmuxDomain;
use emulator::platform::tmux::{TmuxHostConfig, TmuxSocket};
use std::fs;
use std::io::Write as _;
use std::path::Path;
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use tempfile::NamedTempFile;

static COUNTER: AtomicU64 = AtomicU64::new(0);

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

fn ssh(host: &str, port: u16, password: &str, askpass: &str, args: &[&str]) -> String {
    let mut password_file = NamedTempFile::new().expect("create askpass file");
    password_file
        .write_all(password.as_bytes())
        .and_then(|()| password_file.flush())
        .expect("write askpass file");
    let known_hosts = if cfg!(windows) { "NUL" } else { "/dev/null" };
    let mut cmd = Command::new("ssh");
    cmd.arg("-o")
        .arg("StrictHostKeyChecking=accept-new")
        .arg("-o")
        .arg(format!("UserKnownHostsFile={known_hosts}"))
        .arg("-p")
        .arg(port.to_string())
        .arg(host);
    cmd.args(args);
    cmd.env("SSH_ASKPASS", askpass)
        .env("SSH_ASKPASS_REQUIRE", "force")
        .env("XSCORE_ASKPASS_FILE", password_file.path())
        .env("DISPLAY", "localhost:0");
    let output = cmd.output().expect("run ssh");
    String::from_utf8_lossy(&output.stdout).into_owned()
}

#[tokio::test]
#[ignore = "requires test.config with a reachable SSH host"]
async fn ssh_tmux_attach_split_kill_sync() {
    let config = TestConfig::load();
    let host = &config.ssh_host;
    let port = config.ssh_port;
    let password = &config.ssh_password;
    let askpass = std::env::var("CARGO_BIN_EXE_xs")
        .expect("Cargo should provide CARGO_BIN_EXE_xs for this integration test");

    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let session = format!("xscore-it-{}-{n}", std::process::id());

    // 清场并新建一个专用 session。
    let _ = ssh(
        host,
        port,
        password,
        &askpass,
        &["tmux", "kill-session", "-t", &session],
    );
    let _ = ssh(
        host,
        port,
        password,
        &askpass,
        &[
            "tmux",
            "new-session",
            "-d",
            "-s",
            &session,
            "-x",
            "100",
            "-y",
            "30",
        ],
    );

    let host_config = TmuxHostConfig::Ssh {
        destination: host.to_owned(),
        args: vec![
            "-o".to_string(),
            format!(
                "UserKnownHostsFile={}",
                if cfg!(windows) { "NUL" } else { "/dev/null" }
            ),
            "-p".to_string(),
            port.to_string(),
        ],
        password: Some(password.to_owned()),
        askpass: Some(askpass.clone()),
    };
    let domain = TmuxDomain::new();
    let panes = domain
        .attach(&host_config, &TmuxSocket::Default, &session)
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

    // 清理。
    let _ = ssh(
        host,
        port,
        password,
        &askpass,
        &["tmux", "kill-session", "-t", &session],
    );
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
