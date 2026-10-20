// crates/xscore/tests/data_smoke.rs (v0.4.0)
//
// End-to-end smoke for the data path, using the public `SessionManager`
// API directly (not the CLI) so a full
//   add_link → (pane) → data_write → data_read
// round-trip happens in one process. This catches the bugs the CLI
// multi-process tests can't (buffer lifetime, detach semantics, I/O
// timing).
//
// v0.4.0 focus: the pane's buffer (DirectBridge + RingBuffer) really
// holds bytes, detach does NOT kill the link's transport, and tmux links
// drop their panes along with the session.
//
// IMPORTANT: these tests use `std::env::set_var` for `XSCORE_SOCKET`
// and `HOME` (the manager reads XSCORE_SOCKET at sink-creation time and
// HOME for the state file path). Env vars are PROCESS-GLOBAL, so tests
// MUST be run serially:
//
//   cargo test -p xscore -- --test-threads=1
//
// The cli_smoke tests don't have this problem because they use
// `Command::env(...)` per Command.

use std::time::Duration;
use xscore::core::SessionManager;
use xscore::url;

fn unique_socket_name(tag: &str) -> String {
    format!("xscore-test-data-{tag}-{}", std::process::id())
}

async fn cleanup_socket(name: &str) {
    let _ = tokio::process::Command::new("tmux")
        .args(["-L", name, "kill-server"])
        .output().await;
}

/// Point the process at a fresh HOME (state.json) and a per-test tmux
/// socket, then make sure no stale server is left from a previous run.
async fn setup(tag: &str) -> String {
    let sock = unique_socket_name(tag);
    std::env::set_var("XSCORE_SOCKET", &sock);
    let home = std::env::temp_dir().join(format!("xscore-data-test-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&home);
    std::fs::create_dir_all(&home).ok();
    std::env::set_var("HOME", &home);
    cleanup_socket(&sock).await;
    sock
}

async fn read_all(mgr: &SessionManager, pid: xscore::core::PaneId) -> String {
    let mut buf = vec![0u8; 8192];
    let n = mgr.data_read(pid, &mut buf).await.expect("data_read");
    String::from_utf8_lossy(&buf[..n]).to_string()
}

// ----- (kept) base round-trip on a direct link -----

#[tokio::test]
async fn data_read_after_link_pane_attach_write() {
    let sock = setup("base").await;

    let mgr = SessionManager::new();
    let (spec, addr) = url::parse("shell:///bin/cat").expect("parse shell:///bin/cat");
    let res = mgr.add_link(spec, addr, None, None).await.expect("add_link");
    let pane_id = res.pane_id;
    assert!(res.window_id.is_some(), "a direct link now creates a virtual window");

    // Write to the pane's buffer; cat echoes it to its stdout (the source).
    mgr.data_write(pane_id, b"hello from data_smoke\n").await
        .expect("data_write");
    tokio::time::sleep(Duration::from_millis(300)).await;

    let n = mgr.data_read(pane_id, &mut vec![0u8; 4096]).await.expect("data_read");
    assert!(n > 0, "expected non-empty read after write, got {n} bytes");

    // Detach (orphan the pane) and re-attach: the link must still work.
    mgr.detach_pane(pane_id).await.expect("detach_pane");
    mgr.attach_pane(pane_id, res.link_id).await.expect("re-attach_pane");

    mgr.del_link(res.link_id).await.expect("del_link");
    cleanup_socket(&sock).await;
}

// ----- (kept, adapted) tmux link hosts several panes at once -----

#[tokio::test]
async fn tmux_link_supports_multiple_attached_panes() {
    // Q1 corollary: tmux-backed links may have multiple panes attached
    // simultaneously — no auto-detach, and every pane carries its own
    // %N handle.
    let sock = setup("tmux-multi").await;

    let mgr = SessionManager::new();
    let (spec, addr) = url::parse("shell:///bin/cat?tmux=on").expect("parse");
    let res = mgr.add_link(spec, addr, None, None).await.expect("add_link");
    let wid = res.window_id.expect("tmux link adopts a window");

    let p2 = mgr.add_pane(None, 80, 24, 0, 0, Some(wid)).await.expect("p2");
    let p3 = mgr.add_pane(None, 80, 24, 0, 0, Some(wid)).await.expect("p3");

    let mut handles = Vec::new();
    for pid in [res.pane_id, p2, p3] {
        let info = mgr.panes.get(pid).await.expect("pane info");
        assert_eq!(info.link_id, Some(res.link_id),
            "tmux link should keep pane {pid} attached");
        assert_eq!(info.window_id, Some(wid));
        let h = info.sink_handle.expect("tmux pane handle");
        assert!(h.starts_with('%'), "expected %N, got {h}");
        handles.push(h);
    }
    handles.sort();
    handles.dedup();
    assert_eq!(handles.len(), 3, "three distinct %N handles expected: {handles:?}");

    mgr.del_link(res.link_id).await.expect("del_link");
    cleanup_socket(&sock).await;
}

// ----- (kept) Q1: direct link keeps one active pane -----

#[tokio::test]
async fn direct_link_auto_detaches_previously_active_pane() {
    let sock = setup("q1").await;

    let mgr = SessionManager::new();
    let (spec, addr) = url::parse("shell:///bin/cat").expect("parse"); // default = raw
    let res = mgr.add_link(spec, addr, None, None).await.expect("add_link");
    let p1 = res.pane_id; // auto-created, attached

    let p2 = mgr.add_pane(None, 80, 24, 0, 0, None).await.expect("p2");
    mgr.attach_pane(p2, res.link_id).await.expect("attach p2");

    let info_p1 = mgr.panes.get(p1).await.expect("info_p1");
    assert_eq!(info_p1.link_id, None,
        "Q1: p1 should be auto-detached; got {:?}", info_p1.link_id);
    let info_p2 = mgr.panes.get(p2).await.expect("info_p2");
    assert_eq!(info_p2.link_id, Some(res.link_id),
        "p2 should be the active pane; got {:?}", info_p2.link_id);

    mgr.del_link(res.link_id).await.expect("del_link");
    cleanup_socket(&sock).await;
}

// ----- (kept) state round-trip across two managers -----

#[tokio::test]
async fn round_trip_across_two_managers() {
    let sock = setup("roundtrip").await;

    // Manager A: add link (raw; brings its own pane), write, persist.
    let mgr_a = SessionManager::new();
    let (spec, addr) = url::parse("shell:///bin/cat").expect("parse");
    let res = mgr_a.add_link(spec, addr, None, None).await.expect("a.add_link");
    let link_id_a = res.link_id;
    let pane_id_a = res.pane_id;
    mgr_a.data_write(pane_id_a, b"persistence round-trip\n").await.expect("a.data_write");
    tokio::time::sleep(Duration::from_millis(200)).await;

    // Manager B: hydrate from disk, verify it sees the link + pane.
    let mgr_b = SessionManager::load_from_state().await.expect("b.load");
    let arc_links = mgr_b.links.list().await;
    assert_eq!(arc_links.len(), 1, "expected 1 hydrated link, got {}", arc_links.len());
    assert_eq!(arc_links[0].id, link_id_a, "hydrated link id mismatch");

    let info_b = mgr_b.panes.get(pane_id_a).await.expect("b.pane_info");
    assert_eq!(info_b.id, pane_id_a, "hydrated pane id mismatch");
    // v0.4.1 (P0-3): the *binding* survives a restart, but the
    // *connection* does not — a direct transport dies with the process
    // that spawned it. `link_id` stays `Some(..)` (the pane is still
    // bound to that link) and data ops report `LinkNotConnected`
    // instead of the misleading `NotAttached`.
    assert_eq!(info_b.link_id, Some(link_id_a),
        "hydrated pane keeps its binding; got {:?}", info_b.link_id);

    // Direct panes cannot rebuild their bridge (the transport died with
    // the old process) — data ops fail until re-attach, and they must
    // say WHY (v0.4.1 / P0-3). "Not attached" would be a lie: the pane
    // is still bound; the *connection* is gone.
    let res = mgr_b.data_read(pane_id_a, &mut [0u8; 16]).await;
    let err = format!("{}", res.expect_err("data_read on hydrated direct pane must fail"));
    assert!(
        err.contains("not connected") || err.contains("LinkNotConnected"),
        "expected LinkNotConnected, got: {err}"
    );

    let _ = mgr_a.del_link(link_id_a).await;
    let _ = mgr_b.del_link(link_id_a).await;
    cleanup_socket(&sock).await;
}

// ----- (kept) orphan panes reject data ops -----

#[tokio::test]
async fn orphan_pane_data_ops_return_not_attached() {
    let mgr = SessionManager::new();
    let pane_id = mgr.add_pane(None, 80, 24, 0, 0, None).await.expect("add_pane");

    let mut buf = [0u8; 16];
    let res = mgr.data_read(pane_id, &mut buf).await;
    assert!(res.is_err(), "data_read on orphan pane must fail");
    let msg = format!("{}", res.unwrap_err());
    assert!(
        msg.contains("not attached") || msg.contains("NotAttached"),
        "expected NotAttached error, got: {msg}"
    );

    assert!(mgr.data_write(pane_id, b"hi").await.is_err(),
        "data_write on orphan pane must fail");
    assert!(mgr.data_resize(pane_id, 100, 30).await.is_err(),
        "data_resize on orphan pane must fail");
}

// ===== v0.4.0 additions =====

/// #14 — the DirectBridge buffer really captures bytes, and the bytes
/// survive a detach (the buffer belongs to the pane, not the bridge).
#[tokio::test]
async fn direct_bridge_buffer_retains_after_detach() {
    let sock = setup("buffer").await;
    let mgr = SessionManager::new();
    let (spec, addr) = url::parse("shell:///bin/cat").expect("parse");
    let res = mgr.add_link(spec, addr, None, None).await.expect("add_link");
    let pid = res.pane_id;

    mgr.data_write(pid, b"hello\n").await.expect("data_write");
    tokio::time::sleep(Duration::from_millis(300)).await;

    let first = read_all(&mgr, pid).await;
    assert!(first.contains("hello"),
        "the pane buffer should hold the echoed bytes, got {first:?}");

    // A buffer read is a snapshot, not a drain: reading again still sees it.
    let second = read_all(&mgr, pid).await;
    assert!(second.contains("hello"), "buffer read must be non-destructive, got {second:?}");

    // Detach: the pane has no bridge, so data ops are refused...
    mgr.detach_pane(pid).await.expect("detach_pane");
    assert!(mgr.data_read(pid, &mut [0u8; 64]).await.is_err(),
        "a detached pane has no live bridge");

    // ...but re-attaching resumes the SAME buffer, history included.
    mgr.attach_pane(pid, res.link_id).await.expect("re-attach");
    let after = read_all(&mgr, pid).await;
    assert!(after.contains("hello"),
        "buffer must be preserved across detach/re-attach, got {after:?}");

    mgr.del_link(res.link_id).await.expect("del_link");
    cleanup_socket(&sock).await;
}

/// #15 — P0-a: detaching must not kill the link's transport.
#[tokio::test]
async fn detach_keeps_transport_alive() {
    let sock = setup("detach-alive").await;
    let mgr = SessionManager::new();
    let (spec, addr) = url::parse("shell:///bin/cat").expect("parse");
    let res = mgr.add_link(spec, addr, None, None).await.expect("add_link");
    let pid = res.pane_id;

    mgr.detach_pane(pid).await.expect("detach_pane");
    mgr.attach_pane(pid, res.link_id).await.expect("re-attach_pane");

    // If detach had closed the transport, this write fails outright.
    let written = mgr.data_write(pid, b"still alive\n").await
        .expect("data_write after detach must succeed");
    assert_eq!(written, b"still alive\n".len());

    tokio::time::sleep(Duration::from_millis(300)).await;
    let out = read_all(&mgr, pid).await;
    assert!(out.contains("still alive"),
        "the re-attached bridge should read the echo, got {out:?}");

    mgr.del_link(res.link_id).await.expect("del_link");
    cleanup_socket(&sock).await;
}

/// #16 — del_link leaves direct panes in place as orphans.
#[tokio::test]
async fn link_del_direct_keeps_pane_orphan() {
    let sock = setup("del-direct").await;
    let mgr = SessionManager::new();
    let (spec, addr) = url::parse("shell:///bin/cat").expect("parse");
    let res = mgr.add_link(spec, addr, None, None).await.expect("add_link");
    let pid = res.pane_id;

    mgr.del_link(res.link_id).await.expect("del_link");

    let info = mgr.panes.get(pid).await.expect("pane must survive del_link");
    assert_eq!(info.link_id, None, "pane should be orphaned, got {:?}", info.link_id);
    assert!(mgr.links.get(res.link_id).await.is_none(), "link must be gone");
    assert!(mgr.data_read(pid, &mut [0u8; 8]).await.is_err(),
        "orphaned pane has no bridge");
    cleanup_socket(&sock).await;
}

/// #17 — del_link on a tmux link removes its panes/windows (session death).
#[tokio::test]
async fn link_del_tmux_removes_panes() {
    let sock = setup("del-tmux").await;
    let mgr = SessionManager::new();
    let (spec, addr) = url::parse("shell:///bin/cat?tmux=on").expect("parse");
    let res = mgr.add_link(spec, addr, None, None).await.expect("add_link");
    let pid = res.pane_id;
    let wid = res.window_id.expect("adopted window");

    mgr.del_link(res.link_id).await.expect("del_link");

    assert!(mgr.panes.get(pid).await.is_none(), "tmux pane must be removed with the link");
    assert!(mgr.windows.get(wid).await.is_none(), "adopted window must be removed too");
    assert!(mgr.links.get(res.link_id).await.is_none());

    cleanup_socket(&sock).await;
}

/// #18 — `sync_link` is scoped to its own session: healing link A must
/// never touch link B's windows/panes.
#[tokio::test]
async fn sync_link_does_not_touch_other_links() {
    let sock = setup("sync-scope").await;
    let mgr = SessionManager::new();

    let (spec_a, addr_a) = url::parse("shell:///bin/cat?tmux=on").expect("parse a");
    let a = mgr.add_link(spec_a, addr_a, None, None).await.expect("add a");
    let (spec_b, addr_b) = url::parse("shell:///bin/cat?tmux=on").expect("parse b");
    let b = mgr.add_link(spec_b, addr_b, None, None).await.expect("add b");

    let report = mgr.sync_link(a.link_id, true).await.expect("sync a");
    assert!(report.stale_windows.is_empty(),
        "link A sync must not see link B's windows as stale: {report:?}");
    assert!(report.stale_panes.is_empty(), "{report:?}");

    assert!(mgr.panes.get(b.pane_id).await.is_some(),
        "link B's pane must survive link A's heal");
    assert!(mgr.windows.get(b.window_id.unwrap()).await.is_some(),
        "link B's window must survive link A's heal");
    assert!(mgr.panes.get(a.pane_id).await.is_some(), "link A's pane stays");

    mgr.del_link(a.link_id).await.expect("del a");
    mgr.del_link(b.link_id).await.expect("del b");
    cleanup_socket(&sock).await;
}
