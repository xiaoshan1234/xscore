#![cfg(windows)]

use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};
use std::{io::Write, process::Output};

struct Daemon {
    child: Child,
    exe: std::path::PathBuf,
    home: std::path::PathBuf,
}

impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = Command::new(&self.exe)
            .args(["daemon", "stop"])
            .env("HOME", &self.home)
            .env("XDG_RUNTIME_DIR", "")
            .output();
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn command(exe: &Path, home: &Path, args: &[&str]) -> Command {
    let mut command = Command::new(exe);
    command
        .args(args)
        .env("HOME", home)
        .env("XDG_RUNTIME_DIR", "");
    command
}

fn start_daemon(exe: &Path, home: &Path) -> Daemon {
    let child = command(exe, home, &["daemon", "serve"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("start daemon");
    Daemon {
        child,
        exe: exe.to_path_buf(),
        home: home.to_path_buf(),
    }
}

fn wait_for_daemon(exe: &Path, home: &Path) -> Output {
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut last = None;
    while Instant::now() < deadline {
        let out = command(exe, home, &["daemon", "status"])
            .output()
            .expect("daemon status");
        if out.status.success() {
            last = Some(out);
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    last.expect("daemon did not answer over the named pipe")
}

fn id_after(output: &Output, key: &str) -> String {
    let prefix = format!("{key}:");
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .find_map(|line| line.trim().strip_prefix(&prefix))
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .unwrap_or_else(|| panic!("missing `{prefix}` in {:?}", output.stdout))
        .to_string()
}

#[test]
fn named_pipe_daemon_status_and_stop() {
    let exe = std::path::PathBuf::from(env!("CARGO_BIN_EXE_xscore"));
    let home = tempfile::tempdir().expect("temp HOME");
    let mut daemon = start_daemon(&exe, home.path());

    let out = wait_for_daemon(&exe, home.path());
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).expect("daemon status JSON");
    assert_eq!(json["ok"], true);
    assert!(json["status"]["pid"].is_u64());

    let stop = command(&exe, home.path(), &["daemon", "stop"])
        .output()
        .expect("daemon stop");
    assert!(
        stop.status.success(),
        "{}",
        String::from_utf8_lossy(&stop.stderr)
    );
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline && daemon.child.try_wait().expect("wait").is_none() {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(
        daemon.child.try_wait().expect("wait").is_some(),
        "daemon did not exit after Stop"
    );
}

#[test]
fn powershell_session_roundtrips_through_shell_transport() {
    let exe = std::path::PathBuf::from(env!("CARGO_BIN_EXE_xscore"));
    let home = tempfile::tempdir().expect("temp HOME");
    let _daemon = start_daemon(&exe, home.path());
    wait_for_daemon(&exe, home.path());

    let created = command(
        &exe,
        home.path(),
        &[
            "session",
            "new",
            "ps",
            "shell:///C:/Windows/System32/WindowsPowerShell/v1.0/powershell.exe?arg=-NoLogo",
        ],
    )
    .output()
    .expect("create PowerShell session");
    assert!(
        created.status.success(),
        "{}",
        String::from_utf8_lossy(&created.stderr)
    );
    let pane = id_after(&created, "pane");

    std::thread::sleep(Duration::from_millis(300));
    let mut write = command(&exe, home.path(), &["data", "write", "--pane", &pane])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn data write");
    write
        .stdin
        .take()
        .expect("write stdin")
        .write_all(b"Write-Output XSCORE_POWERSHELL_OK\r\n")
        .expect("write PowerShell command");
    let written = write.wait_with_output().expect("finish data write");
    assert!(
        written.status.success(),
        "{}",
        String::from_utf8_lossy(&written.stderr)
    );

    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let read = command(
            &exe,
            home.path(),
            &["data", "read", "--pane", &pane, "--bytes", "4096"],
        )
        .output()
        .expect("data read");
        assert!(
            read.status.success(),
            "{}",
            String::from_utf8_lossy(&read.stderr)
        );
        if String::from_utf8_lossy(&read.stdout).contains("XSCORE_POWERSHELL_OK") {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "PowerShell output did not arrive"
        );
        std::thread::sleep(Duration::from_millis(100));
    }
}
