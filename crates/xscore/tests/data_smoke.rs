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
// holds bytes, and detach does NOT kill the link's transport.
// v0.6.0 (ADR 0010): a tmux session is driven by ONE control link that serves
// every pane of the session (`link_key = %N`), and `session kill` is the only
// cascade that takes those rows down.
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

#![cfg(unix)]

use std::time::Duration;
use xscore::core::{SessionManager, TmuxCreateOptions};
use xscore::model::{TmuxHostConfig, TmuxSocket};
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
    mgr.add_session_native("test").await.expect("session");
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

// ----- one control link serves every pane of its tmux session -----

#[tokio::test]
async fn one_control_link_serves_every_pane_of_its_session() {
    // ADR 0010 §D3: sharing a link is ADDRESSING here, not a race. Three panes
    // of one tmux session ride ONE `Control` link, each with its own `%N`, so
    // there is no auto-detach — and exactly one link row.
    let sock = setup("tmux-multi").await;
    let _guard = SocketGuard(sock.clone());
    let mgr = SessionManager::new();
    let sid = mgr
        .create_tmux_session("multi", TmuxHostConfig::Local, TmuxSocket::Named(sock.clone()), 80, 24)
        .await
        .expect("create tmux session");

    let window = mgr
        .windows
        .list_all()
        .await
        .into_iter()
        .find(|w| w.info.session_id == sid && w.info.backend.tmux_handle().is_some())
        .expect("the session's tmux window");
    let wid = window.info.id;
    let first = mgr
        .panes
        .list_all()
        .await
        .into_iter()
        .find(|p| p.session_id == sid)
        .expect("the session's first pane")
        .id;

    let p2 = mgr.add_pane_in(sid, None, 0, 0, 0, 0, Some(wid)).await.expect("split 1");
    let p3 = mgr.add_pane_in(sid, None, 0, 0, 0, 0, Some(wid)).await.expect("split 2");

    // One link for the session, and it is the control client.
    let links: Vec<_> = mgr
        .links
        .list()
        .await
        .into_iter()
        .filter(|l| l.info.session_id == sid)
        .collect();
    assert_eq!(links.len(), 1, "a tmux session has exactly one link");
    assert!(links[0].info.is_control(), "and it is the control link: {:?}", links[0].info);
    let lid = links[0].id;

    let mut keys = Vec::new();
    for pid in [first, p2, p3] {
        let info = mgr.panes.get(pid).await.expect("pane info");
        assert_eq!(info.link_id, Some(lid), "every pane rides the one link: {info:?}");
        assert_eq!(info.window_id, Some(wid), "and lives in the one tmux window");
        let key = info.link_key.expect("a control pane is addressed by its %N");
        assert!(key.starts_with('%'), "expected %N, got {key}");
        keys.push(key);
    }
    keys.sort();
    keys.dedup();
    assert_eq!(keys.len(), 3, "three distinct %N addresses expected: {keys:?}");

    mgr.del_session(sid).await.expect("kill the session");
    cleanup_socket(&sock).await;
}

// ----- (kept) Q1: direct link keeps one active pane -----

#[tokio::test]
async fn direct_link_auto_detaches_previously_active_pane() {
    let sock = setup("q1").await;

    let mgr = SessionManager::new();
    mgr.add_session_native("test").await.expect("session");
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
    mgr_a.add_session_native("test").await.expect("session");
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
    mgr.add_session_native("test").await.expect("session");
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
    mgr.add_session_native("test").await.expect("session");
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
    mgr.add_session_native("test").await.expect("session");
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
    mgr.add_session_native("test").await.expect("session");
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

/// #17 — `session kill` is the only cascade: the tmux panes, windows and link
/// go with the session, and so does the server session itself.
#[tokio::test]
async fn session_kill_removes_the_tmux_panes() {
    let sock = setup("del-tmux").await;
    let _guard = SocketGuard(sock.clone());
    let mgr = SessionManager::new();
    let sid = mgr
        .create_tmux_session("killme", TmuxHostConfig::Local, TmuxSocket::Named(sock.clone()), 80, 24)
        .await
        .expect("create tmux session");
    let pane = mgr
        .panes
        .list_all()
        .await
        .into_iter()
        .find(|p| p.session_id == sid)
        .expect("the session's pane")
        .id;
    let wid = mgr
        .windows
        .list_all()
        .await
        .into_iter()
        .find(|w| w.info.session_id == sid)
        .expect("the session's window")
        .info
        .id;
    let lid = mgr
        .links
        .list()
        .await
        .into_iter()
        .find(|l| l.info.session_id == sid)
        .expect("the session's control link")
        .id;

    mgr.del_session(sid).await.expect("kill the session");

    assert!(mgr.panes.get(pane).await.is_none(), "the tmux pane row must be removed");
    assert!(mgr.windows.get(wid).await.is_none(), "its window row too");
    assert!(mgr.links.get(lid).await.is_none(), "and the control link");
    assert!(mgr.sessions.get(sid).await.is_none());

    // The server session is the backend: kill means kill.
    let alive = tokio::process::Command::new("tmux")
        .args(["-L", &sock, "has-session", "-t", "killme"])
        .output()
        .await
        .expect("tmux has-session");
    assert!(!alive.status.success(), "`session kill` must end the tmux session");

    cleanup_socket(&sock).await;
}

// ----- v0.6.0 (PR3b, ADR 0010 §D2): a control-managed session -----
//
// These have to run in-process: the point is the DAEMON-side machinery (the
// `%output` pump and the event pump), and a CLI invocation is a different
// process each time, so it could only ever observe the rendered screen.

/// Kills this test's tmux server on drop, so a failing assertion does not leave
/// a server (and a pane's shell) running after the suite.
struct SocketGuard(String);

impl Drop for SocketGuard {
    fn drop(&mut self) {
        let _ = std::process::Command::new("tmux")
            .args(["-L", &self.0, "kill-server"])
            .output();
    }
}

/// The pane's buffer is filled by `%output`, not by `capture-pane`. The two are
/// distinguishable: a 10-row pane renders about ten lines, while the buffer
/// keeps everything the pane wrote.
#[tokio::test]
async fn tmux_control_output_pump_fills_the_pane_buffer() {
    let sock = setup("tmux-pump").await;
    let _guard = SocketGuard(sock.clone());
    let mgr = SessionManager::new();
    let sid = mgr
        .create_tmux_session_with(
            "pump",
            TmuxHostConfig::Local,
            TmuxSocket::Named(sock.clone()),
            80,
            10,
            // The delay is for the attach: tmux replays nothing, so the burst
            // has to start after the client has subscribed.
            TmuxCreateOptions { command: Some("sleep 0.5; seq 1 200; sleep 30".into()), cwd: None },
        )
        .await
        .expect("create tmux session");

    let pane = mgr
        .panes
        .list_all()
        .await
        .into_iter()
        .find(|p| p.session_id == sid)
        .expect("the session's pane");

    let mut text = String::new();
    for _ in 0..100 {
        text = read_all(&mgr, pane.id).await;
        if text.contains("200\r\n") {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    // In order, from the first line: the buffer is a stream, so it has what the
    // screen scrolled away.
    assert!(text.starts_with("1\r\n"), "buffer should begin at line 1: {:?}", &text[..40.min(text.len())]);
    assert!(text.contains("200\r\n"), "the pump stopped early: {:?}", &text[text.len().saturating_sub(80)..]);
    assert!(
        text.lines().count() > 20,
        "only {} lines: this looks like a capture-pane snapshot, not the stream",
        text.lines().count()
    );

    mgr.del_session(sid).await.expect("kill the session");
    cleanup_socket(&sock).await;
}

/// A window created by someone else becomes a row through the EVENT pump: no
/// refresh pass runs here, which is the whole claim of arch.md §7.
#[tokio::test]
async fn tmux_window_add_event_becomes_a_window_row() {
    let sock = setup("tmux-window-add").await;
    let _guard = SocketGuard(sock.clone());
    let mgr = SessionManager::new();
    let sid = mgr
        .create_tmux_session("events", TmuxHostConfig::Local, TmuxSocket::Named(sock.clone()), 80, 10)
        .await
        .expect("create tmux session");
    assert_eq!(rows(&mgr, sid).await.len(), 1, "a new tmux session has one window");

    // Outside xscore, from another process.
    let added = tokio::process::Command::new("tmux")
        .args(["-L", &sock, "new-window", "-d", "-t", "events", "-n", "extra"])
        .output()
        .await
        .expect("tmux new-window");
    assert!(added.status.success(), "tmux new-window failed");

    let mut found = Vec::new();
    let mut in_window = Vec::new();
    for _ in 0..100 {
        found = rows(&mgr, sid).await;
        // The window row is written before its panes are enumerated (they take
        // one more round trip), so wait for both rather than asserting on the
        // intermediate state.
        in_window = match found.iter().find(|w| w.info.name == "extra") {
            Some(window) => mgr
                .panes
                .list_all()
                .await
                .iter()
                .filter(|p| p.window_id == Some(window.info.id))
                .cloned()
                .collect(),
            None => Vec::new(),
        };
        if found.len() == 2 && in_window.len() == 1 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    assert_eq!(found.len(), 2, "expected the new window as a row, got {found:?}");
    let extra = found
        .iter()
        .find(|w| w.info.name == "extra")
        .expect("the window is named after tmux's");
    assert!(
        extra.info.backend.tmux_handle().map(|h| h.starts_with('@')).unwrap_or(false),
        "the row must mirror @N: {:?}",
        extra.info.backend
    );
    // ...and tmux's window has a pane row of its own, addressed by `%N`.
    assert_eq!(in_window.len(), 1, "the new window's pane was not adopted");
    assert!(
        in_window[0].link_key.as_deref().map(|k| k.starts_with('%')).unwrap_or(false),
        "the adopted pane must carry its %N as link_key: {:?}",
        in_window[0].link_key
    );

    mgr.del_session(sid).await.expect("kill the session");
    cleanup_socket(&sock).await;
}

/// The session's window rows, in a stable order.
async fn rows(mgr: &SessionManager, sid: xscore::core::SessionId) -> Vec<std::sync::Arc<xscore::core::window::Window>> {
    let mut windows: Vec<_> = mgr
        .windows
        .list_all()
        .await
        .into_iter()
        .filter(|w| w.info.session_id == sid)
        .collect();
    windows.sort_by_key(|w| w.info.created_at);
    windows
}
