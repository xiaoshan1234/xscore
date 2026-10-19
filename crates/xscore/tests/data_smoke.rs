// crates/xscore/tests/data_smoke.rs
//
// End-to-end smoke for the data path (v0.3.0).
//
// Uses the public `SessionManager` API directly (not the CLI) so we can
// exercise the full
//   add_link → add_pane → attach_pane → data_write → data_read
// round-trip in one process. This catches the bugs that the CLI
// multi-process tests can't (state-persistence races, cross-invocation
// I/O timing).
//
// Each test uses a unique `XSCORE_SOCKET` (so its tmux server doesn't
// collide with other tests) and cleans up the server on entry + exit.
//
// IMPORTANT: these tests use `std::env::set_var` for `XSCORE_SOCKET`
// and `HOME` (the manager reads XSCORE_SOCKET at sink-creation time
// and HOME for the state file path). Env vars are PROCESS-GLOBAL, so
// tests MUST be run serially:
//
//   cargo test -p xscore --test data_smoke -- --test-threads=1
//
// Otherwise two parallel tests will race on the env var and one will
// write state to the other's HOME (the failure manifests as
// "rename ... No such file or directory" on `add_link`'s save_state).
// The cli_smoke tests don't have this problem because they use
// `Command::env(...)` per-Command; data_smoke doesn't spawn the
// binary, it uses the in-process manager which reads env directly.

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
async fn data_read_after_link_pane_attach_write() {
    // Full v0.3.0 flow: add_link (raw), add_pane (orphan),
    // attach_pane (creates bridge), data_write, data_read.
    let sock = unique_socket_name();
    std::env::set_var("XSCORE_SOCKET", &sock);
    cleanup_socket(&sock).await;

    let mgr = SessionManager::new();
    let (spec, addr) = url::parse("shell:///bin/cat").expect("parse shell:///bin/cat");
    let link_id = mgr.add_link(spec, addr, None).await.expect("add_link");

    let pane_id = mgr.add_pane(None, 80, 24, 0, 0).await.expect("add_pane");
    mgr.attach_pane(pane_id, link_id).await.expect("attach_pane");

    // Write to the bridge; cat echoes it to its stdout (the source).
    mgr.data_write(pane_id, b"hello from data_smoke\n").await
        .expect("data_write");

    // Give the shell a moment to process the input and for the
    // transport to flush. 300ms is empirical for local tmux / cat.
    tokio::time::sleep(Duration::from_millis(300)).await;

    let mut buf = vec![0u8; 4096];
    let n = mgr.data_read(pane_id, &mut buf).await.expect("data_read");
    let captured = String::from_utf8_lossy(&buf[..n]);

    // The exact contents are too transport-/shell-specific to assert
    // on. The reliable assertion is that the write + read round-trip
    // doesn't error and returns SOMETHING (the pipe is non-empty
    // after the write).
    assert!(n > 0, "expected non-empty read after write, got {n} bytes: {captured:?}");

    // Detach (orphan the pane), then re-attach. The pane's buffer
    // content is preserved (it's the transport's stdin buffer, not
    // the bridge's).
    mgr.detach_pane(pane_id).await.expect("detach_pane");
    mgr.attach_pane(pane_id, link_id).await.expect("re-attach_pane");

    // Cleanup
    mgr.del_link(link_id).await.expect("del_link");
    cleanup_socket(&sock).await;
}

#[tokio::test]
async fn tmux_link_supports_multiple_attached_panes() {
    // Q1 corollary: tmux-backed links (with `?tmux=on`) may have
    // multiple panes attached simultaneously — no auto-detach on
    // subsequent attach calls.
    let sock = unique_socket_name();
    std::env::set_var("XSCORE_SOCKET", &sock);
    cleanup_socket(&sock).await;

    let mgr = SessionManager::new();
    let (spec, addr) = url::parse("shell:///bin/cat?tmux=on").expect("parse");
    let link_id = mgr.add_link(spec, addr, None).await.expect("add_link");

    let p1 = mgr.add_pane(None, 80, 24, 0, 0).await.expect("p1");
    let p2 = mgr.add_pane(None, 80, 24, 0, 0).await.expect("p2");

    mgr.attach_pane(p1, link_id).await.expect("attach p1");
    mgr.attach_pane(p2, link_id).await.expect("attach p2");

    // Both panes should be attached. p1's info.link_id is unchanged
    // (no Q1 auto-detach for tmux links).
    let info_p1 = mgr.panes.get(p1).await.expect("info_p1");
    assert_eq!(info_p1.link_id, Some(link_id),
        "tmux link should keep both panes attached; p1.link_id = {:?}", info_p1.link_id);
    let info_p2 = mgr.panes.get(p2).await.expect("info_p2");
    assert_eq!(info_p2.link_id, Some(link_id),
        "tmux link should keep both panes attached; p2.link_id = {:?}", info_p2.link_id);

    // Cleanup
    mgr.del_pane(p1).await.expect("del_pane p1");
    mgr.del_pane(p2).await.expect("del_pane p2");
    mgr.del_link(link_id).await.expect("del_link");
    cleanup_socket(&sock).await;
}

#[tokio::test]
async fn direct_link_auto_detaches_previously_active_pane() {
    // Q1: a direct (raw) link may have only ONE active pane at a time.
    // When a different pane is attached, the previously-active pane is
    // auto-detached (its info.link_id becomes None).
    let sock = unique_socket_name();
    std::env::set_var("XSCORE_SOCKET", &sock);
    cleanup_socket(&sock).await;

    let mgr = SessionManager::new();
    let (spec, addr) = url::parse("shell:///bin/cat").expect("parse");  // default = raw
    let link_id = mgr.add_link(spec, addr, None).await.expect("add_link");

    let p1 = mgr.add_pane(None, 80, 24, 0, 0).await.expect("p1");
    let p2 = mgr.add_pane(None, 80, 24, 0, 0).await.expect("p2");

    mgr.attach_pane(p1, link_id).await.expect("attach p1");
    // p1 is now active. Attach p2 → p1 should be auto-detached (Q1).
    mgr.attach_pane(p2, link_id).await.expect("attach p2");

    let info_p1 = mgr.panes.get(p1).await.expect("info_p1");
    assert_eq!(info_p1.link_id, None,
        "Q1: p1 should be auto-detached; got {:?}", info_p1.link_id);
    let info_p2 = mgr.panes.get(p2).await.expect("info_p2");
    assert_eq!(info_p2.link_id, Some(link_id),
        "p2 should be the active pane; got {:?}", info_p2.link_id);

    // Cleanup
    mgr.del_pane(p1).await.expect("del_pane p1");
    mgr.del_pane(p2).await.expect("del_pane p2");
    mgr.del_link(link_id).await.expect("del_link");
    cleanup_socket(&sock).await;
}

#[tokio::test]
async fn round_trip_across_two_managers() {
    // Two SessionManagers, one for "session A" (adds link + pane, attaches, writes),
    // one for "session B" (hydrates from disk). On hydration, panes are
    // preserved but no bridge is built (the user must re-attach).
    let sock = unique_socket_name();
    std::env::set_var("XSCORE_SOCKET", &sock);
    cleanup_socket(&sock).await;

    let home = std::env::temp_dir().join(format!("xscore-data-test-{}-{}",
        std::process::id(), uuid::Uuid::now_v7()));
    std::fs::create_dir_all(&home).ok();
    std::env::set_var("HOME", &home);

    // Manager A: add link (raw), add pane, attach, write, persist.
    let mgr_a = SessionManager::new();
    let (spec, addr) = url::parse("shell:///bin/cat").expect("parse");  // raw
    let link_id_a = mgr_a.add_link(spec, addr, None).await.expect("a.add_link");
    let pane_id_a = mgr_a.add_pane(None, 80, 24, 0, 0).await.expect("a.add_pane");
    mgr_a.attach_pane(pane_id_a, link_id_a).await.expect("a.attach_pane");
    mgr_a.data_write(pane_id_a, b"persistence round-trip\n").await.expect("a.data_write");
    tokio::time::sleep(Duration::from_millis(200)).await;

    // Manager B: hydrate from disk, verify it sees the link + pane.
    let mgr_b = SessionManager::load_from_state().await.expect("b.load");
    let arc_links = mgr_b.links.list().await;
    assert_eq!(arc_links.len(), 1, "expected 1 hydrated link, got {}", arc_links.len());
    let arc_link_b = &arc_links[0];
    assert_eq!(arc_link_b.id, link_id_a, "hydrated link id mismatch");

    // The pane's info should round-trip (link_id preserved on info,
    // but bridge is None — user must re-attach after restart).
    let info_b = mgr_b.panes.get(pane_id_a).await.expect("b.pane_info");
    assert_eq!(info_b.id, pane_id_a, "hydrated pane id mismatch");
    assert_eq!(info_b.link_id, Some(link_id_a),
        "hydrated pane should preserve link_id; got {:?}", info_b.link_id);

    // data ops on hydrated pane: link_id is Some but bridge is None.
    // With the spec's "no auto-bridge on hydrate" design, the bridge
    // is None → data_read returns NotAttached. We accept that.
    let res = mgr_b.data_read(pane_id_a, &mut [0u8; 16]).await;
    assert!(res.is_err(), "data_read on hydrated pane (no bridge) must fail");

    // Cleanup A and B.
    let _ = mgr_a.del_link(link_id_a).await;
    let _ = mgr_b.del_link(link_id_a).await;
    cleanup_socket(&sock).await;
}

#[tokio::test]
async fn orphan_pane_data_ops_return_not_attached() {
    // An orphan pane (no link attached) must reject data ops with
    // `Error::NotAttached(pid)`.
    let mgr = SessionManager::new();
    let pane_id = mgr.add_pane(None, 80, 24, 0, 0).await.expect("add_pane");

    let mut buf = [0u8; 16];
    let res = mgr.data_read(pane_id, &mut buf).await;
    assert!(res.is_err(), "data_read on orphan pane must fail");
    let err = res.unwrap_err();
    let msg = format!("{err}");
    assert!(
        msg.contains("not attached") || msg.contains("NotAttached"),
        "expected NotAttached error, got: {msg}"
    );

    // data_write and data_resize also fail for orphans.
    let res = mgr.data_write(pane_id, b"hi").await;
    assert!(res.is_err(), "data_write on orphan pane must fail");
    let res = mgr.data_resize(pane_id, 100, 30).await;
    assert!(res.is_err(), "data_resize on orphan pane must fail");
}