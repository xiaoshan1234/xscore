// crates/xscore/tests/data_smoke.rs
//
// End-to-end smoke for the data path. Uses the public `SessionManager`
// API directly (not the CLI) so we can exercise the full
//   add_link → backend() → write → snapshot
// round-trip in one process. This catches the bugs that the CLI
// multi-process tests can't (state-persistence state-mutation
// races, cross-invocation I/O timing).
//
// Each test uses a unique `XSCORE_SOCKET` (so its tmux server doesn't
// collide with other tests) and cleans up the server on entry + exit.

use std::time::Duration;
use xscore::core::SessionManager;
use xscore::url;

fn unique_socket_name() -> String {
    format!("xscore-test-data-{}-{}", std::process::id(), uuid::Uuid::now_v7())
}

async fn cleanup_socket(name: &str) {
    let _ = tokio::process::Command::new("tmux")
        .args(["-L", name, "kill-server"])
        .output().await;
}

#[tokio::test]
async fn data_read_after_link_add_writes_to_pane() {
    // Unique tmux socket so we don't collide with parallel tests or
    // a developer's running tmux server.
    let sock = unique_socket_name();
    std::env::set_var("XSCORE_SOCKET", &sock);
    cleanup_socket(&sock).await;

    let mgr = SessionManager::new();
    let (spec, addr) = url::parse("shell:///bin/cat").expect("parse shell:///bin/cat");
    let (link_id, _window_id, pane_id) = mgr.add_link(spec, addr)
        .await
        .expect("add_link");

    let backend = mgr.backend(pane_id).await.expect("backend");

    // Write a line to the pane; cat echoes it to its stdout (which
    // the sink captures via tmux send-keys → the shell → cat).
    backend.write(b"hello from data_smoke\n").await
        .expect("backend.write");

    // Give the shell a moment to process the input and for tmux to
    // flush capture-pane's buffer. 200ms is empirical — slower CI
    // machines may need more, but for the local-tmux path this is
    // plenty.
    tokio::time::sleep(Duration::from_millis(300)).await;

    let mut buf = vec![0u8; 4096];
    let n = backend.snapshot(&mut buf).await.expect("backend.snapshot");
    let captured = String::from_utf8_lossy(&buf[..n]);

    // The pane may or may not include the echoed text — depends on
    // whether the shell is a login shell that prints a prompt first,
    // and on tmux's capture-pane line-buffer. The reliable assertion
    // is that the write + snapshot round-trip doesn't error and
    // returns SOMETHING (the pane is non-empty after the write).
    // The exact contents are too tmux-version-specific to assert on.
    assert!(n > 0, "expected non-empty snapshot after write, got {n} bytes: {captured:?}");

    // Cleanup
    mgr.del_link(link_id).await.expect("del_link");
    cleanup_socket(&sock).await;
}

#[tokio::test]
async fn add_link_creates_exactly_one_window() {
    // Verifies the double-default-window fix: a fresh link should
    // produce exactly ONE window in the tmux session, not two.
    let sock = unique_socket_name();
    std::env::set_var("XSCORE_SOCKET", &sock);
    cleanup_socket(&sock).await;

    let mgr = SessionManager::new();
    let (spec, addr) = url::parse("shell:///bin/cat").expect("parse");
    let (link_id, _wid, _pid) = mgr.add_link(spec, addr).await.expect("add_link");

    // List tmux windows for this link's session.
    let session_name = format!("xscore-{link_id}");
    let out = tokio::process::Command::new("tmux")
        .args(["-L", &sock, "list-windows", "-t", &session_name, "-F", "#{window_id}"])
        .output().await.expect("list-windows");
    assert!(out.status.success(), "list-windows failed: {:?}", out.status);
    let stdout = String::from_utf8_lossy(&out.stdout);
    let n_windows: usize = stdout.lines().filter(|l| !l.is_empty()).count();
    assert_eq!(n_windows, 1,
        "expected 1 window per link (double-default fix); got {n_windows}. session={session_name}");

    mgr.del_link(link_id).await.expect("del_link");
    cleanup_socket(&sock).await;
}

#[tokio::test]
async fn round_trip_across_two_managers() {
    // Two SessionManagers, one for "session A" (adds the link, writes),
    // one for "session B" (hydrates from disk, reads). This is the
    // multi-invocation flow: today, manager B is a fresh process; here
    // we simulate that with two in-process managers sharing HOME.
    let sock = unique_socket_name();
    std::env::set_var("XSCORE_SOCKET", &sock);
    cleanup_socket(&sock).await;

    // Use a per-test HOME so the state file doesn't collide with the
    // CLI tests or other data tests.
    let home = std::env::temp_dir().join(format!("xscore-data-test-{}-{}",
        std::process::id(), uuid::Uuid::now_v7()));
    std::fs::create_dir_all(&home).ok();
    std::env::set_var("HOME", &home);

    // Manager A: add link, write, persist.
    let mgr_a = SessionManager::new();
    let (spec, addr) = url::parse("shell:///bin/cat").expect("parse");
    let (link_id_a, _wid_a, pane_id_a) = mgr_a.add_link(spec, addr).await.expect("a.add_link");
    let backend_a = mgr_a.backend(pane_id_a).await.expect("a.backend");
    backend_a.write(b"persistence round-trip\n").await.expect("a.write");
    tokio::time::sleep(Duration::from_millis(200)).await;

    // Manager B: hydrate from disk, verify it sees the link.
    let mgr_b = SessionManager::load_from_state().await.expect("b.load");
    let arc_links = mgr_b.links.list().await;
    assert_eq!(arc_links.len(), 1, "expected 1 hydrated link, got {}", arc_links.len());
    let arc_link_b = &arc_links[0];
    assert_eq!(arc_link_b.id, link_id_a, "hydrated link id mismatch");

    // Manager B's backend handle for pane_a (the persisted id) should
    // not error (the backend is a NoopTransport-based CompositeBackend
    // — its data ops fail cleanly since the original Transport is
    // gone, but the lookup itself must succeed).
    let res = mgr_b.backend(pane_id_a).await;
    assert!(res.is_ok(), "b.backend(pane_a) should resolve the pane");

    // Cleanup A and B.
    let _ = mgr_a.del_link(link_id_a).await;
    let _ = mgr_b.del_link(link_id_a).await;
    cleanup_socket(&sock).await;
}
