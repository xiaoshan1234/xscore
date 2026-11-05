//! TmuxDomain 端到端：SSH control client、输入输出、resize、split、枚举与 kill。

mod common;

use common::{receive_until, TestConfig};
use emulator::domain::{PaneHandle, TmuxDomain};
use emulator::platform::tmux::{TmuxHostConfig, TmuxSocket};
use emulator::platform::Size;
use std::io::Write as _;
use std::path::PathBuf;
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;
use tempfile::NamedTempFile;

static COUNTER: AtomicU64 = AtomicU64::new(0);

const TMUX_OK: &[u8] = b"XSCORE_TMUX_DOMAIN_OK";

struct RemoteSession {
    config: TestConfig,
    askpass: PathBuf,
    name: String,
}

impl RemoteSession {
    fn create(config: TestConfig, askpass: PathBuf) -> Self {
        let id = COUNTER.fetch_add(1, Ordering::Relaxed);
        let name = format!("xscore-it-{}-{id}", std::process::id());
        let session = Self {
            config,
            askpass,
            name,
        };

        let output = session
            .ssh(&[
                "tmux",
                "new-session",
                "-d",
                "-s",
                &session.name,
                "-x",
                "100",
                "-y",
                "30",
            ])
            .expect("run ssh to create tmux session");
        assert!(
            output.status.success(),
            "failed to create tmux session: {}",
            String::from_utf8_lossy(&output.stderr)
        );

        session
    }

    fn ssh(&self, args: &[&str]) -> std::io::Result<Output> {
        let mut password_file = NamedTempFile::new()?;
        password_file.write_all(self.config.ssh_password.as_bytes())?;
        password_file.flush()?;
        let mut command = Command::new("ssh");
        command
            .args(ssh_options(&self.config))
            .arg(&self.config.ssh_destination)
            .args(args)
            .env("SSH_ASKPASS", &self.askpass)
            .env("SSH_ASKPASS_REQUIRE", "force")
            .env("XSCORE_ASKPASS_FILE", password_file.path())
            .env("DISPLAY", "localhost:0");
        command.output()
    }
}

impl Drop for RemoteSession {
    fn drop(&mut self) {
        let _ = self.ssh(&["tmux", "kill-session", "-t", &self.name]);
    }
}

fn ssh_options(config: &TestConfig) -> Vec<String> {
    let known_hosts = if cfg!(windows) { "NUL" } else { "/dev/null" };
    vec![
        "-o".to_string(),
        "StrictHostKeyChecking=accept-new".to_string(),
        "-o".to_string(),
        format!("UserKnownHostsFile={known_hosts}"),
        "-p".to_string(),
        config.ssh_port.to_string(),
    ]
}

fn askpass_binary() -> PathBuf {
    if let Some(path) = std::env::var_os("XSCORE_TEST_ASKPASS") {
        return PathBuf::from(path);
    }

    let executable = std::env::current_exe().expect("resolve integration test executable");
    let target_dir = executable
        .parent()
        .and_then(|path| path.parent())
        .expect("resolve target directory");
    let candidate = target_dir.join(if cfg!(windows) { "xs.exe" } else { "xs" });
    assert!(
        candidate.is_file(),
        "xs askpass helper not found at {}; run scripts/run_integration_tests.py",
        candidate.display()
    );
    candidate
}

#[tokio::test]
#[ignore = "requires test.config and an xs askpass helper"]
async fn tmux_ssh_io_resize_split_kill() {
    let config = TestConfig::load();
    let askpass = askpass_binary();
    let session = RemoteSession::create(config.clone(), askpass.clone());

    let host = TmuxHostConfig::Ssh {
        destination: config.ssh_destination.clone(),
        args: ssh_options(&config),
        password: Some(config.ssh_password.clone()),
        askpass: Some(askpass.display().to_string()),
    };
    let domain = TmuxDomain::new();
    let panes = domain
        .attach(&host, &TmuxSocket::Default, &session.name)
        .await
        .expect("attach to tmux session");

    assert!(!panes.is_empty(), "expected at least one tmux pane");
    let pane = &panes[0];
    let mut output = pane.subscribe();
    pane.send_input(b"printf 'XSCORE_%s\\n' 'TMUX_DOMAIN_OK'\n")
        .await
        .expect("send input to tmux pane");

    let received = receive_until(&mut output, TMUX_OK, Duration::from_secs(10)).await;
    let text = String::from_utf8_lossy(&received);
    assert!(
        received
            .windows(TMUX_OK.len())
            .any(|window| window == TMUX_OK),
        "expected tmux output marker, got: {text:?}"
    );

    pane.resize(Size::new(90, 25))
        .await
        .expect("resize tmux pane");

    let window = pane.window().to_string();
    let reply = pane
        .send_command(&format!("split-window -d -t {window} -P -F '#{{pane_id}}'"))
        .await
        .expect("split tmux window");
    assert_eq!(reply.len(), 1, "split-window should print one pane id");
    let new_pane = reply[0].trim().to_string();

    let total = pane
        .enumerate_windows()
        .await
        .expect("enumerate tmux windows")
        .iter()
        .map(|window| window.panes.len())
        .sum::<usize>();
    assert_eq!(total, 2, "expected two panes after split");

    pane.send_command(&format!("kill-pane -t {new_pane}"))
        .await
        .expect("kill split tmux pane");
    let total = pane
        .enumerate_windows()
        .await
        .expect("enumerate tmux windows after kill")
        .iter()
        .map(|window| window.panes.len())
        .sum::<usize>();
    assert_eq!(total, 1, "expected one pane after kill");
}
