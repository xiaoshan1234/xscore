//! Live smoke test for the tmux control client.
//!
//! Ignored by default: it needs a real `tmux` (3.x) on `PATH`. The in-crate
//! tests cover the protocol against a captured transcript and a fake `tmux`,
//! but two classes of bug only appear against a real server, and both of them
//! did:
//!
//!   * `-CC` dies on pipes (`tcgetattr failed`), so the client must use `-C`;
//!   * tmux numbers `%begin` with its own counter, so replies must be matched
//!     by order — a client that waits for its own `num` hangs forever.
//!
//! Run it with:
//!
//!     cargo test -p xscore --test tmux_control_smoke -- --ignored --nocapture
//!
//! It uses a private socket and kills it on the way out, so it never touches
//! the user's tmux server.

#![cfg(unix)]

use xscore::model::TmuxSocket;
use xscore::tmux::TmuxControl;

/// Kills the private server even when the test panics.
struct Guard {
    sock: String,
}

impl Drop for Guard {
    fn drop(&mut self) {
        let _ = std::process::Command::new("tmux")
            .args(["-L", &self.sock, "kill-server"])
            .status();
    }
}

#[tokio::test]
#[ignore = "needs a live tmux 3.x; run with --ignored"]
async fn control_client_streams_bytes_runs_commands_and_receives_events() {
    let sock = format!("xsctl-smoke-{}", std::process::id());
    let _guard = Guard { sock: sock.clone() };
    let session = "probe";

    // Keeps producing output, so `%output` has to arrive *after* we attach —
    // tmux replays nothing.
    let started = std::process::Command::new("tmux")
        .args([
            "-L",
            &sock,
            "new-session",
            "-d",
            "-s",
            session,
            "sh",
            "-c",
            "while :; do echo TICK; sleep 0.1; done",
        ])
        .status()
        .expect("start tmux");
    assert!(started.success(), "tmux new-session failed");

    let socket = TmuxSocket::Named(sock.clone());
    let control = TmuxControl::spawn(&[], &socket, session).expect("spawn control client");

    // 1. Pane bytes arrive as a stream.
    let mut pane = control.output("%0").await.expect("subscribe pane");
    let first = tokio::time::timeout(std::time::Duration::from_secs(10), pane.recv())
        .await
        .expect("no %output within 10s — the client never attached")
        .expect("pane channel closed");
    let text = String::from_utf8_lossy(&first).to_string();
    assert!(text.contains("TICK"), "unexpected pane bytes: {text:?}");

    // 2. Commands are answered on the same channel. tmux picks the `num`, so
    //    this also proves the order-based correlation.
    let windows = control
        .send("list-windows -F '#{window_id} #{window_width}x#{window_height}'")
        .await
        .expect("send list-windows");
    assert!(
        windows.iter().any(|l| l.starts_with("@0 ")),
        "expected an @0 line, got {windows:?}"
    );

    // 2b. A reply line can itself start with `%` — a pane id does. The reader
    //     used to classify every `%`-prefixed line as an unknown notification
    //     and drop it, so `list-panes` answered with an empty vec against a
    //     live server while its one-shot twin worked. This is that bug's
    //     regression test.
    let panes = control
        .send(&xscore::tmux::list_panes_command("%0"))
        .await
        .expect("send list-panes");
    assert!(
        panes.iter().any(|line| line.starts_with("%0\t")),
        "a `%`-prefixed reply line was dropped: {panes:?}"
    );

    // 3. Structure arrives without being asked.
    let mut events = control.subscribe();
    std::thread::spawn({
        let sock = sock.clone();
        move || {
            std::thread::sleep(std::time::Duration::from_millis(150));
            let _ = std::process::Command::new("tmux")
                .args(["-L", &sock, "new-window", "-t", "probe", "-n", "second"])
                .status();
        }
    });
    let mut saw_add = false;
    for _ in 0..20 {
        match tokio::time::timeout(std::time::Duration::from_millis(500), events.recv()).await {
            Ok(Some(ev)) => {
                if matches!(ev, xscore::tmux::TmuxEvent::WindowAdd { .. }) {
                    saw_add = true;
                    break;
                }
            }
            Ok(None) => break,
            Err(_) => continue,
        }
    }
    assert!(saw_add, "no %window-add event observed");

    control.close().await.expect("close");
    assert!(control.is_closed(), "close must mark the client closed");
}
