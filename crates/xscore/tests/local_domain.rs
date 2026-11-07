//! LocalDomain 端到端：PTY 子进程输出经 TerminalHandle 路由回来。

use tokio::time::{timeout, Duration};
use xscore::domain::{Domain, LocalDomain, TerminalConfig};

#[tokio::test]
async fn local_pane_roundtrip() {
    let (program, args): (&str, Vec<&str>) = if cfg!(windows) {
        ("cmd.exe", vec!["/c", "echo %XSCORE_PTY_ENV%"])
    } else {
        ("sh", vec!["-c", "printf '%s\\n' \"$XSCORE_PTY_ENV\""])
    };

    let domain = LocalDomain::new();
    let handle = domain
        .spawn_terminal(&TerminalConfig::Local {
            command: program.to_string(),
            args: args.into_iter().map(str::to_string).collect(),
            cwd: None,
            env: vec![("XSCORE_PTY_ENV".into(), "XSCORE_PTY_ENV_OK".into())],
            cols: 80,
            rows: 24,
        })
        .await
        .unwrap();

    let mut rx = handle.subscribe();
    let mut seen: Vec<u8> = Vec::new();
    let needle = b"XSCORE_PTY_ENV_OK";
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
        text.contains("XSCORE_PTY_ENV_OK"),
        "expected marker in output, got: {text:?}"
    );
}
