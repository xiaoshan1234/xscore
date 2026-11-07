//! TmuxDomain 端到端：SSH control client、输入输出、resize、split、枚举与 kill。

mod common;

use common::{receive_until, ssh_exec, TestConfig};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;
use xscore::domain::{PaneHandle, TmuxDomain};
use xscore::platform::tmux::{TmuxHostConfig, TmuxSocket};
use xscore::platform::Size;

static COUNTER: AtomicU64 = AtomicU64::new(0);

const TMUX_OK: &[u8] = b"XSCORE_TMUX_DOMAIN_OK";

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
async fn tmux_ssh_io_resize_split_kill() {
    let config = TestConfig::load();
    let session = RemoteSession::create(config.clone()).await;

    let host = TmuxHostConfig::Ssh {
        user: config.ssh_user.clone(),
        host: config.ssh_host.clone(),
        port: config.ssh_port,
        identity: None,
        password: Some(config.ssh_password.clone()),
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
