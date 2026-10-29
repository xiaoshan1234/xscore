//! LocalDomain 端到端：PTY 子进程输出经 PaneHandle 路由回来。

use emulator::domain::{Domain, LocalDomain, PaneConfig};
use tokio::time::{timeout, Duration};

#[tokio::test]
async fn local_pane_roundtrip() {
    let (program, args): (&str, Vec<&str>) = if cfg!(windows) {
        ("cmd.exe", vec!["/c", "echo EMULATOR_PTY_OK"])
    } else {
        ("sh", vec!["-c", "echo EMULATOR_PTY_OK"])
    };

    let domain = LocalDomain::new();
    let handle = domain
        .spawn_pane(&PaneConfig {
            command: program.to_string(),
            args: args.into_iter().map(str::to_string).collect(),
            cwd: None,
            cols: 80,
            rows: 24,
        })
        .await
        .unwrap();

    let mut rx = handle.subscribe();
    let mut seen: Vec<u8> = Vec::new();
    let needle = b"EMULATOR_PTY_OK";
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while tokio::time::Instant::now() < deadline {
        match timeout(Duration::from_millis(200), rx.recv()).await {
            Ok(Ok(chunk)) => {
                seen.extend_from_slice(&chunk);
                if seen.windows(needle.len()).any(|w| w == needle) {
                    break;
                }
            }
            Ok(Err(_)) => break,
            Err(_) => continue,
        }
    }

    let text = String::from_utf8_lossy(&seen);
    assert!(
        text.contains("EMULATOR_PTY_OK"),
        "expected marker in output, got: {text:?}"
    );
}
