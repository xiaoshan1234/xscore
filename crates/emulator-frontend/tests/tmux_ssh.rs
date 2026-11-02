//! 真实环境 tmux 集成测试：SSH 主机上的 attach / split / kill-pane / 结构枚举。
//!
//! 环境变量：
//!   XSCORE_SSH_HOST        目标主机，例如 loner@shanclaw（未设置则跳过）
//!   XSCORE_SSH_PASSWORD 或 PASSWD  密码
//!
//! 运行：
//!   cargo test -p emulator-frontend --test tmux_ssh -- --ignored --nocapture

use emulator::domain::TmuxDomain;
use emulator::platform::tmux::{TmuxHostConfig, TmuxSocket};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

static COUNTER: AtomicU64 = AtomicU64::new(0);

fn env_or_skip(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|s| !s.is_empty())
}

fn ssh_env(cmd: &mut Command, password: &str, askpass: &str) {
    cmd.env("SSH_ASKPASS", askpass)
        .env("SSH_ASKPASS_REQUIRE", "force")
        .env("XSCORE_SSH_PASSWORD", password)
        .env("XSCORE_ASKPASS", "1")
        .env("DISPLAY", "localhost:0");
}

fn ssh(host: &str, password: &str, askpass: &str, args: &[&str]) -> String {
    let mut cmd = Command::new("ssh");
    cmd.arg("-o")
        .arg("StrictHostKeyChecking=accept-new")
        .arg(host);
    cmd.args(args);
    ssh_env(&mut cmd, password, askpass);
    let output = cmd.output().expect("run ssh");
    String::from_utf8_lossy(&output.stdout).into_owned()
}

#[tokio::test]
#[ignore = "requires a real SSH host; set XSCORE_SSH_HOST and XSCORE_SSH_PASSWORD"]
async fn ssh_tmux_attach_split_kill_sync() {
    let Some(host) = env_or_skip("XSCORE_SSH_HOST") else {
        eprintln!("skip: XSCORE_SSH_HOST not set");
        return;
    };
    let Some(password) = env_or_skip("XSCORE_SSH_PASSWORD").or_else(|| env_or_skip("PASSWD"))
    else {
        eprintln!("skip: password env not set");
        return;
    };
    let Some(askpass) = env_or_skip("CARGO_BIN_EXE_xs") else {
        eprintln!("skip: CARGO_BIN_EXE_xs unavailable");
        return;
    };

    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let session = format!("xscore-it-{}-{n}", std::process::id());

    // 清场并新建一个专用 session。
    let _ = ssh(
        &host,
        &password,
        &askpass,
        &["tmux", "kill-session", "-t", &session],
    );
    let _ = ssh(
        &host,
        &password,
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
        destination: host.clone(),
        args: vec![],
        password: Some(password.clone()),
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
        &host,
        &password,
        &askpass,
        &["tmux", "kill-session", "-t", &session],
    );
}
