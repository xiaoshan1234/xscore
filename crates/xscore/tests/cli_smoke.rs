// crates/xscore/tests/cli_smoke.rs (v0.5.0)
//
// CLI smoke tests — run the actual `xscore` binary and verify its
// stdout / stderr / exit code. `env!("CARGO_BIN_EXE_xscore")` is
// provided by Cargo for integration tests.
//
// Isolation: every test gets its own HOME (state.json + the daemon
// socket) AND its own `XSCORE_SOCKET` (so tmux sessions never touch the
// user's `xscore` server or another test's sessions). The HOME is wiped
// once per test name per process, then left alone — multi-invocation
// tests rely on state.json persisting between calls.
//
// v0.5.0 (ADR 0009) semantics under test:
//   - a direct link's transport and pane buffer live in a resident
//     daemon, so they survive the CLI process that created them
//   - `daemon status` / `daemon stop` are the handle on that process
//   - `link show` reports `connected` by probing the daemon live
//
// v0.4.0 semantics still under test (ADR 0008):
//   - `link add` always yields link + (usually) window + pane
//   - `link add --pane` binds a hollow pane and refuses panes already attached
//     elsewhere
//
// v0.6.0 semantics (ADR 0010): a tmux session is created/adopted at the SESSION
// level, has exactly one (control) link, addresses its panes by `%N` in
// `link_key`, keeps its windows in `WindowBackend::Tmux`, refuses `link add`
// and `pane detach`, and loses all of it to `session kill`.

use std::collections::HashSet;
use std::path::PathBuf;
use std::process::Command;
use std::sync::{Mutex, OnceLock};

/// Base directory for per-test HOMEs.
///
/// Unix socket paths are capped at ~104 bytes (`sun_path`), and the
/// daemon socket lives at `$HOME/.config/xscore/run/daemon.sock` (~31
/// bytes of suffix). A long TMPDIR (a cache dir, say) would push the
/// socket path past that limit, so fall back to the short system temp
/// directory. The per-test HOME isolation itself is unchanged.
fn temp_base() -> PathBuf {
    let t = std::env::temp_dir();
    if t.as_os_str().len() > 24 {
        PathBuf::from("/tmp")
    } else {
        t
    }
}

fn home_for(test_name: &str) -> PathBuf {
    temp_base().join(format!("xscore-test-{test_name}"))
}

/// Per-process record of HOMEs already wiped, so repeated calls within
/// one multi-invocation test keep their state.
fn wiped() -> &'static Mutex<HashSet<PathBuf>> {
    static WIPED: OnceLock<Mutex<HashSet<PathBuf>>> = OnceLock::new();
    WIPED.get_or_init(|| Mutex::new(HashSet::new()))
}

fn xscore_for(test_name: &str) -> Command {
    let home = home_for(test_name);
    std::fs::create_dir_all(&home).ok();
    let first_touch = {
        let mut g = wiped().lock().unwrap();
        g.insert(home.clone())
    };
    if first_touch {
        // A daemon from an earlier run may still be holding this test's
        // socket (a daemon with links never idle-exits, by design). Stop
        // it BEFORE its HOME is wiped, so every run starts from a clean
        // slate and no stray daemon outlives the suite.
        let _ = xscore_bin(test_name).args(["daemon", "stop"]).output();
        let _ = std::fs::remove_dir_all(home.join(".config").join("xscore"));
    }
    xscore_bin(test_name)
}

/// The raw command for a test's HOME/socket, with no wipe side effects.
fn xscore_bin(test_name: &str) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_xscore"));
    cmd.env("HOME", home_for(test_name));
    cmd.env("XSCORE_SOCKET", format!("xscore-test-{test_name}"));
    // The daemon socket must live under this test's HOME. A developer
    // shell (or WSL) often exports XDG_RUNTIME_DIR, which would move the
    // socket somewhere every test shares — drop it so the documented
    // HOME fallback applies.
    cmd.env_remove("XDG_RUNTIME_DIR");
    cmd
}

/// Kill this test's tmux server (best effort).
fn cleanup_tmux(test_name: &str) {
    let _ = Command::new("tmux")
        .args(["-L", &format!("xscore-test-{test_name}"), "kill-server"])
        .output();
}

/// Kill this test's daemon (best effort).
///
/// Called at the START of daemon tests (so no daemon from an earlier run
/// can answer) and at the END (so a daemon holding a link — which never
/// idle-exits, by design — doesn't outlive the test). Errors are ignored:
/// "no daemon" is the state we're asking for.
fn cleanup_daemon(test_name: &str) {
    let _ = xscore_bin(test_name).args(["daemon", "stop"]).output();
}

/// Extract the value after a leading "key:" prefix on a single line.
fn parse_id_from_stdout(stdout: &[u8], key: &str) -> String {
    let prefix = format!("{key}:");
    String::from_utf8_lossy(stdout)
        .lines()
        .find(|l| l.trim_start().starts_with(&prefix))
        .unwrap_or_else(|| panic!("no `{prefix}:` line in stdout: {}",
            String::from_utf8_lossy(stdout)))
        .trim()
        .strip_prefix(&prefix)
        .unwrap_or("")
        .trim()
        .to_string()
}

fn json(stdout: &[u8]) -> serde_json::Value {
    serde_json::from_slice(stdout).unwrap_or_else(|e| {
        panic!("expected JSON on stdout, got {:?} ({e})",
            String::from_utf8_lossy(stdout))
    })
}

fn ok(out: &std::process::Output) -> bool {
    if !out.status.success() {
        eprintln!("stderr: {}", String::from_utf8_lossy(&out.stderr));
    }
    out.status.success()
}

#[test]
fn xscore_help_exits_zero() {
    let xscore = || xscore_for("help_exits_zero");
    let out = xscore().arg("--help").output().unwrap();
    assert!(out.status.success());
    let s = String::from_utf8(out.stdout).unwrap();
    assert!(s.contains("link"));
    assert!(s.contains("pane"));
    assert!(s.contains("data"));
    assert!(s.contains("config"));
    assert!(s.contains("window"), "missing 'window' in --help: {s}");
}

// ----- Input validation (no state needed) -----

#[test]
fn xscore_link_add_requires_url_or_config() {
    let xscore = || xscore_for("link_add_requires_url_or_config");
    let out = xscore().args(["link", "add"]).output().unwrap();
    assert!(!out.status.success(), "`link add` with no inputs must fail");
}

// ----- `link config` template (disk-based, no in-memory state needed) -----

#[test]
fn xscore_link_config_add_and_list_roundtrip() {
    let xscore = || xscore_for("link_config_roundtrip");
    let _ = std::fs::remove_dir_all(home_for("link_config_roundtrip").join(".config").join("xscore").join("links"));
    let add = xscore()
        .args(["link", "config", "add", "web", "ssh://root@host:22"])
        .output()
        .unwrap();
    assert!(
        ok(&add),
        "link config add failed: {}",
        String::from_utf8_lossy(&add.stderr)
    );

    let list = xscore()
        .args(["link", "config", "list"])
        .output()
        .unwrap();
    let s = String::from_utf8_lossy(&list.stdout);
    assert!(s.contains("web"), "expected name 'web' in config list, got: {s}");

    let _ = xscore()
        .args(["link", "config", "remove", "web"])
        .output();
}

// ----- ADR 0005 invariant: two `link add <same url>` = two distinct links -----

#[test]
fn xscore_link_add_default_creates_two_links_for_same_url() {
    let xscore = || xscore_for("link_add_two_same_url");
    let out1 = xscore()
        .args(["link", "add", "shell:///bin/echo"])
        .output()
        .unwrap();
    let id1 = parse_id_from_stdout(&out1.stdout, "link");
    let out2 = xscore()
        .args(["link", "add", "shell:///bin/echo"])
        .output()
        .unwrap();
    let id2 = parse_id_from_stdout(&out2.stdout, "link");
    assert_ne!(id1, id2, "two add() calls should produce distinct link_ids");
    cleanup_daemon("link_add_two_same_url");
}

// ----- v0.4.0 #1: direct link auto-creates window + pane -----

#[test]
fn xscore_link_add_direct_creates_window_and_pane() {
    let xscore = || xscore_for("link_add_direct_auto");
    let out = xscore()
        .args(["link", "add", "shell:///bin/cat"])
        .output()
        .unwrap();
    assert!(ok(&out), "add failed: {}", String::from_utf8_lossy(&out.stderr));

    let link_id = parse_id_from_stdout(&out.stdout, "link");
    let window_id = parse_id_from_stdout(&out.stdout, "window");
    let pane_id = parse_id_from_stdout(&out.stdout, "pane");

    // Pane is attached to the link and lives in the auto-created window.
    let pane = json(&xscore()
        .args(["pane", "show", "--id", &pane_id])
        .output()
        .unwrap()
        .stdout);
    assert_eq!(pane["link_id"], serde_json::json!(link_id));
    assert_eq!(pane["window_id"], serde_json::json!(window_id));
    assert!(pane["link_key"].is_null(), "a pipe pane carries no %N address: {pane}");

    // Window is virtual: no tmux handle.
    let window = json(&xscore()
        .args(["window", "show", "--id", &window_id])
        .output()
        .unwrap()
        .stdout);
    assert_eq!(window["backend"]["kind"], serde_json::json!("native"), "native window: {window}");

    // The link really is a pipe: one byte stream, no tmux in the middle.
    let link = json(&xscore()
        .args(["link", "show", "--id", &link_id])
        .output()
        .unwrap()
        .stdout);
    assert_eq!(
        link["endpoint"]["pipe"]["transport"],
        serde_json::json!("shell"),
        "expected EndpointRef::Pipe(shell): {link}"
    );

    let _ = xscore().args(["link", "del", "--id", &link_id]).output();
    cleanup_daemon("link_add_direct_auto");
}

// ----- v0.4.0 #2: `link add --pane` binds an existing orphan pane -----

#[test]
fn xscore_link_add_direct_with_pane() {
    let xscore = || xscore_for("link_add_direct_with_pane");
    let pane_id = parse_id_from_stdout(
        &xscore().args(["pane", "add"]).output().unwrap().stdout,
        "pane",
    );

    let out = xscore()
        .args(["link", "add", "shell:///bin/cat", "--pane", &pane_id])
        .output()
        .unwrap();
    assert!(ok(&out), "add --pane failed: {}", String::from_utf8_lossy(&out.stderr));
    let s = String::from_utf8_lossy(&out.stdout);
    assert!(s.contains("link:"), "missing link: {s}");
    assert!(s.contains("pane:"), "missing pane: {s}");
    assert!(!s.contains("window:"), "binding an existing pane must not create a window: {s}");
    assert_eq!(parse_id_from_stdout(&out.stdout, "pane"), pane_id);

    let link_id = parse_id_from_stdout(&out.stdout, "link");
    let pane = json(&xscore()
        .args(["pane", "show", "--id", &pane_id])
        .output()
        .unwrap()
        .stdout);
    assert_eq!(pane["link_id"], serde_json::json!(link_id));

    let _ = xscore().args(["link", "del", "--id", &link_id]).output();
    cleanup_daemon("link_add_direct_with_pane");
}

// ----- v0.4.0 #3: a pane already attached elsewhere is never stolen -----

#[test]
fn xscore_link_add_direct_with_attached_pane_errors() {
    let xscore = || xscore_for("link_add_attached_pane");
    let pane_id = parse_id_from_stdout(
        &xscore().args(["pane", "add"]).output().unwrap().stdout,
        "pane",
    );
    let first = xscore()
        .args(["link", "add", "shell:///bin/cat", "--pane", &pane_id])
        .output()
        .unwrap();
    assert!(ok(&first));
    let link_a = parse_id_from_stdout(&first.stdout, "link");

    let second = xscore()
        .args(["link", "add", "shell:///bin/cat", "--pane", &pane_id])
        .output()
        .unwrap();
    assert!(!second.status.success(), "re-binding an attached pane must fail");
    let err = String::from_utf8_lossy(&second.stderr);
    assert!(
        err.contains("already attached") && err.contains(&link_a),
        "expected PaneAlreadyAttached({pane_id}, {link_a}), got: {err}"
    );

    // Still attached to A (no silent steal).
    let pane = json(&xscore()
        .args(["pane", "show", "--id", &pane_id])
        .output()
        .unwrap()
        .stdout);
    assert_eq!(pane["link_id"], serde_json::json!(link_a));

    let _ = xscore().args(["link", "del", "--id", &link_a]).output();
    let _ = xscore().args(["pane", "del", "--id", &pane_id]).output();
    cleanup_daemon("link_add_attached_pane");
}

// ----- ADR 0010 §D2: a tmux session has exactly one link -----
//
// The old per-command tmux adoption test is gone: the session path is the only
// now, and `xscore_session_new_tmux_creates_window_and_pane` covers what it
// asserted (`%N` on the pane, `@N` on the window, geometry adopted from tmux).

#[test]
fn xscore_link_add_inside_a_tmux_session_is_refused() {
    let name = "link_add_in_tmux_session";
    let xscore = || xscore_for(name);
    cleanup_tmux(name);

    let out = xscore()
        .args(["session", "new", "work", "--tmux"])
        .output()
        .unwrap();
    assert!(ok(&out), "session new --tmux failed: {}", String::from_utf8_lossy(&out.stderr));
    let lid = parse_id_from_stdout(&out.stdout, "link");

    // One tmux session, one connection: a second link would be a second
    // mechanism driving the same server session.
    let add = xscore()
        .args(["link", "add", "shell:///bin/cat", "--session", "work"])
        .output()
        .unwrap();
    assert!(!add.status.success(), "a tmux session must not take a second link");
    let err = String::from_utf8_lossy(&add.stderr);
    assert!(
        err.contains("control link") || err.contains("one link"),
        "expected TmuxSessionHasOneLink, got: {err}"
    );

    // `--pane` must not make it legal, and the refusal must leave the pane
    // exactly where it was.
    let orphan = parse_id_from_stdout(
        &xscore().args(["pane", "add"]).output().unwrap().stdout,
        "pane",
    );
    let with_pane = xscore()
        .args(["link", "add", "shell:///bin/cat", "--session", "work", "--pane", &orphan])
        .output()
        .unwrap();
    assert!(!with_pane.status.success(), "`--pane` must not bypass the one-link rule");
    let pane = json(&xscore().args(["pane", "show", "--id", &orphan]).output().unwrap().stdout);
    assert!(pane["link_id"].is_null(), "a rejected add must leave the pane orphan: {pane}");

    // And the session still has exactly one link, as the filter reports.
    let ls = xscore().args(["link", "list", "--session", "work"]).output().unwrap();
    assert!(ok(&ls), "link list failed: {}", String::from_utf8_lossy(&ls.stderr));
    let listing = String::from_utf8_lossy(&ls.stdout).to_string();
    let rows = listing.lines().skip(1).filter(|l| !l.trim().is_empty()).count();
    assert_eq!(rows, 1, "one tmux session, one link: {listing}");
    assert!(listing.contains(&lid), "the control link is the one: {listing}");
    assert!(listing.contains("control"), "and it is reported as such: {listing}");

    let _ = xscore().args(["pane", "del", "--id", &orphan]).output();
    let _ = xscore().args(["session", "kill", "work"]).output();
    cleanup_tmux(name);
}

// ----- v0.4.0 #6: virtual window (no --link) -----

#[test]
fn xscore_window_add_virtual() {
    let xscore = || xscore_for("window_add_virtual");
    let out = xscore()
        .args(["window", "add", "--name", "scratch", "--cols", "100", "--rows", "20"])
        .output()
        .unwrap();
    assert!(ok(&out));
    let wid = parse_id_from_stdout(&out.stdout, "window");
    let w = json(&xscore()
        .args(["window", "show", "--id", &wid])
        .output()
        .unwrap()
        .stdout);
    assert_eq!(w["backend"]["kind"], serde_json::json!("native"), "native window: {w}");
    assert_eq!(w["cols"], serde_json::json!(100));
    assert!(w["name"] == serde_json::json!("scratch"), "{w}");
    let _ = xscore().args(["window", "del", "--id", &wid]).output();
}

// ----- ADR 0010: a window in a tmux session is a real tmux window -----

#[test]
fn xscore_window_add_tmux_session() {
    let name = "window_add_tmux_session";
    let xscore = || xscore_for(name);
    cleanup_tmux(name);

    let created = xscore()
        .args(["session", "new", "work", "--tmux"])
        .output()
        .unwrap();
    assert!(ok(&created), "session new --tmux failed: {}", String::from_utf8_lossy(&created.stderr));

    // No `--link`: the SESSION decides, which is the whole point of ADR 0010.
    let out = xscore()
        .args(["window", "add", "--name", "logs", "--session", "work"])
        .output()
        .unwrap();
    assert!(ok(&out), "window add --session failed: {}", String::from_utf8_lossy(&out.stderr));
    let wid = parse_id_from_stdout(&out.stdout, "window");

    let w = json(&xscore().args(["window", "show", "--id", &wid]).output().unwrap().stdout);
    assert_eq!(w["backend"]["kind"], serde_json::json!("tmux"), "{w}");
    let handle = w["backend"]["handle"].as_str().expect("tmux handle");
    assert!(handle.starts_with('@'), "expected @N, got {handle}");
    assert_eq!(w["name"], serde_json::json!("logs"));

    // tmux is the source of truth: it has the window too.
    let listing = tmux_stdout(name, &["list-windows", "-t", "work", "-F", "#{window_name}"]);
    assert!(listing.contains("logs"), "tmux does not have the window: {listing:?}");

    let _ = xscore().args(["session", "kill", "work"]).output();
    cleanup_tmux(name);
}

// ----- ADR 0010: `pane add --window` splits a real tmux window -----

#[test]
fn xscore_pane_add_to_tmux_window() {
    let name = "pane_add_tmux_window";
    let xscore = || xscore_for(name);
    cleanup_tmux(name);

    let add = xscore()
        .args(["session", "new", "work", "--tmux"])
        .output()
        .unwrap();
    assert!(ok(&add), "session new --tmux failed: {}", String::from_utf8_lossy(&add.stderr));
    let lid = parse_id_from_stdout(&add.stdout, "link");
    let wid = parse_id_from_stdout(&add.stdout, "window");
    let adopted = parse_id_from_stdout(&add.stdout, "pane");

    let out = xscore()
        .args(["pane", "add", "--window", &wid, "--session", "work"])
        .output()
        .unwrap();
    assert!(ok(&out), "pane add --window failed: {}", String::from_utf8_lossy(&out.stderr));
    let pid = parse_id_from_stdout(&out.stdout, "pane");
    assert_ne!(pid, adopted);

    // The new row is tmux's: it is addressed by its own `%N` on the session's
    // ONE link, not by a link of its own.
    let pane = json(&xscore().args(["pane", "show", "--id", &pid]).output().unwrap().stdout);
    let key = pane["link_key"].as_str().expect("link_key");
    assert!(key.starts_with('%'), "expected %N, got {key}");
    assert_eq!(pane["link_id"], serde_json::json!(lid));
    assert_eq!(pane["window_id"], serde_json::json!(wid));

    // Both panes are the session's; no other session sees them.
    let ls = xscore().args(["pane", "list", "--session", "work"]).output().unwrap();
    assert!(ok(&ls), "pane list failed: {}", String::from_utf8_lossy(&ls.stderr));
    let listing = String::from_utf8_lossy(&ls.stdout).to_string();
    let rows = listing.lines().skip(1).filter(|l| !l.trim().is_empty()).count();
    assert_eq!(rows, 2, "expected the adopted pane plus the split: {listing}");
    assert!(listing.contains(&pid) && listing.contains(&adopted), "{listing}");

    let _ = xscore().args(["session", "kill", "work"]).output();
    cleanup_tmux(name);
}

// ----- ADR 0010 §10: the two list verbs take a --session filter -----

#[test]
fn xscore_window_and_pane_list_take_a_session_filter() {
    let name = "list_session_filter";
    let xscore = || xscore_for(name);

    let a = xscore().args(["session", "new", "A"]).output().unwrap();
    assert!(ok(&a), "session new A failed: {}", String::from_utf8_lossy(&a.stderr));
    let wid_a = parse_id_from_stdout(&a.stdout, "window");
    let pid_a = parse_id_from_stdout(&a.stdout, "pane");

    let b = xscore().args(["session", "new", "B"]).output().unwrap();
    assert!(ok(&b), "session new B failed: {}", String::from_utf8_lossy(&b.stderr));
    let wid_b = parse_id_from_stdout(&b.stdout, "window");
    let pid_b = parse_id_from_stdout(&b.stdout, "pane");

    let windows = xscore().args(["window", "list", "--session", "A"]).output().unwrap();
    assert!(ok(&windows), "window list failed: {}", String::from_utf8_lossy(&windows.stderr));
    let wl = String::from_utf8_lossy(&windows.stdout).to_string();
    assert!(wl.contains(&wid_a), "A's window missing: {wl}");
    assert!(!wl.contains(&wid_b), "B's window leaked into A's listing: {wl}");
    assert!(wl.contains("SESSION"), "the listing names the session column: {wl}");

    let panes = xscore().args(["pane", "list", "--session", "A"]).output().unwrap();
    assert!(ok(&panes), "pane list failed: {}", String::from_utf8_lossy(&panes.stderr));
    let pl = String::from_utf8_lossy(&panes.stdout).to_string();
    assert!(pl.contains(&pid_a), "A's pane missing: {pl}");
    assert!(!pl.contains(&pid_b), "B's pane leaked into A's listing: {pl}");

    // No filter still shows both.
    let all = xscore().args(["window", "list"]).output().unwrap();
    let al = String::from_utf8_lossy(&all.stdout).to_string();
    assert!(al.contains(&wid_a) && al.contains(&wid_b), "{al}");

    cleanup_daemon(name);
}

// ----- v0.4.0 #10: `pane add` with no window stays orphan -----

#[test]
fn xscore_pane_add_orphan() {
    let xscore = || xscore_for("pane_add_orphan");
    let out = xscore().args(["pane", "add", "--cols", "90", "--rows", "25"]).output().unwrap();
    assert!(ok(&out));
    let pid = parse_id_from_stdout(&out.stdout, "pane");
    let pane = json(&xscore()
        .args(["pane", "show", "--id", &pid])
        .output()
        .unwrap()
        .stdout);
    assert!(pane["link_id"].is_null(), "{pane}");
    assert!(pane["window_id"].is_null(), "{pane}");
    assert!(pane["link_key"].is_null(), "{pane}");
    assert_eq!(pane["cols"], serde_json::json!(90));
    assert_eq!(pane["rows"], serde_json::json!(25));
    let _ = xscore().args(["pane", "del", "--id", &pid]).output();
}

// ----- v0.4.0 #11: link del orphans direct panes -----

#[test]
fn xscore_link_del_direct_orphans_panes() {
    let xscore = || xscore_for("link_del_direct_orphans");
    let add = xscore().args(["link", "add", "shell:///bin/cat"]).output().unwrap();
    assert!(ok(&add));
    let link_id = parse_id_from_stdout(&add.stdout, "link");
    let pane_id = parse_id_from_stdout(&add.stdout, "pane");

    assert!(ok(&xscore().args(["link", "del", "--id", &link_id]).output().unwrap()));

    let pane = json(&xscore()
        .args(["pane", "show", "--id", &pane_id])
        .output()
        .unwrap()
        .stdout);
    assert!(pane["link_id"].is_null(), "pane should be orphaned: {pane}");
    let _ = xscore().args(["pane", "del", "--id", &pane_id]).output();
    cleanup_daemon("link_del_direct_orphans");
}

// ----- ADR 0010 §8: `session kill` is the cascade that removes tmux panes -----

#[test]
fn xscore_session_kill_removes_tmux_panes() {
    let name = "session_kill_removes_panes";
    let xscore = || xscore_for(name);
    cleanup_tmux(name);

    let add = xscore()
        .args(["session", "new", "work", "--tmux"])
        .output()
        .unwrap();
    assert!(ok(&add), "session new --tmux failed: {}", String::from_utf8_lossy(&add.stderr));
    let pane_id = parse_id_from_stdout(&add.stdout, "pane");

    let kill = xscore().args(["session", "kill", "work"]).output().unwrap();
    assert!(ok(&kill), "session kill failed: {}", String::from_utf8_lossy(&kill.stderr));

    let show = xscore().args(["pane", "show", "--id", &pane_id]).output().unwrap();
    assert!(!show.status.success(), "the tmux pane row must be gone after session kill");
    assert!(
        String::from_utf8_lossy(&show.stderr).contains("pane not found"),
        "expected PaneNotFound, got: {}",
        String::from_utf8_lossy(&show.stderr)
    );
    let windows = xscore().args(["window", "list"]).output().unwrap();
    assert_eq!(
        String::from_utf8_lossy(&windows.stdout).lines().count(),
        1,
        "only the header should remain"
    );
    cleanup_tmux(name);
}

// ----- ADR 0010 §8: a pane on a control link cannot be detached -----

#[test]
fn xscore_tmux_pane_cannot_detach() {
    let name = "tmux_pane_cannot_detach";
    let xscore = || xscore_for(name);
    cleanup_tmux(name);

    let add = xscore()
        .args(["session", "new", "work", "--tmux"])
        .output()
        .unwrap();
    assert!(ok(&add), "session new --tmux failed: {}", String::from_utf8_lossy(&add.stderr));
    let link_id = parse_id_from_stdout(&add.stdout, "link");
    let pane_id = parse_id_from_stdout(&add.stdout, "pane");

    let out = xscore()
        .args(["pane", "detach", "--pane", &pane_id])
        .output()
        .unwrap();
    assert!(!out.status.success(), "detaching a tmux pane must fail");
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        err.contains("owned by tmux") && err.contains("pane del"),
        "expected TmuxPaneNotDetachable, got: {err}"
    );

    // Still attached after the rejected detach.
    let pane = json(&xscore()
        .args(["pane", "show", "--id", &pane_id])
        .output()
        .unwrap()
        .stdout);
    assert_eq!(pane["link_id"], serde_json::json!(link_id));
    assert!(pane["link_key"].as_str().map(|k| k.starts_with('%')).unwrap_or(false),
        "the pane keeps its %N address: {pane}");

    let _ = xscore().args(["session", "kill", "work"]).output();
    cleanup_tmux(name);
}

// ----- v0.4.0: legacy state.json is rejected, not silently emptied -----

#[test]
fn xscore_rejects_legacy_state_schema() {
    let xscore = || xscore_for("legacy_state_schema");
    // First touch wipes this test's HOME, so write the legacy file after.
    assert!(xscore().args(["link", "list"]).output().unwrap().status.success());
    let state = home_for("legacy_state_schema")
        .join(".config")
        .join("xscore")
        .join("state")
        .join("state.json");
    std::fs::create_dir_all(state.parent().unwrap()).unwrap();
    // v0.3.0 shape: three lists, no schema_version.
    std::fs::write(&state, r#"{"links":[],"windows":[],"panes":[]}"#).unwrap();

    let out = xscore().args(["link", "list"]).output().unwrap();
    assert!(!out.status.success(), "legacy state.json must be a hard error");
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("schema_version=0"), "got: {err}");
    // v0.6.0 (ADR 0010 D8): the schema moved to 3, so the message (and this
    // assertion, which tracks it) moved with it.
    assert!(err.contains("expected 3"), "got: {err}");
    assert!(err.contains("delete"), "error should tell the user what to do: {err}");
    assert!(err.contains("state migrate"), "error should name the migration: {err}");

    // Deleting it restores normal operation.
    std::fs::remove_file(&state).unwrap();
    assert!(xscore().args(["link", "list"]).output().unwrap().status.success());
}

// ----- v0.4.0: full flow still works on the decoupled verbs -----

#[test]
fn xscore_full_flow_create_pane_attach_data() {
    let xscore = || xscore_for("full_flow");

    // 1. Create link (raw). It now comes with a pane of its own.
    let add = xscore().args(["link", "add", "shell:///bin/cat"]).output().unwrap();
    assert!(ok(&add));
    let link_id = parse_id_from_stdout(&add.stdout, "link");
    let auto_pane = parse_id_from_stdout(&add.stdout, "pane");

    // 2. Create another pane (orphan) and attach it.
    let add_pane = xscore().args(["pane", "add"]).output().unwrap();
    let pane_id = parse_id_from_stdout(&add_pane.stdout, "pane");

    let attach = xscore()
        .args(["pane", "attach", "--pane", &pane_id, "--link", &link_id])
        .output().unwrap();
    assert!(ok(&attach), "attach failed: {}", String::from_utf8_lossy(&attach.stderr));

    // Q1: the auto-created pane was auto-detached.
    let auto = json(&xscore()
        .args(["pane", "show", "--id", &auto_pane])
        .output()
        .unwrap()
        .stdout);
    assert!(auto["link_id"].is_null(), "Q1 should detach the old pane: {auto}");

    // 3. Verify the pane's info shows the link_id.
    let show = xscore().args(["pane", "show", "--id", &pane_id]).output().unwrap();
    let s = String::from_utf8_lossy(&show.stdout);
    assert!(s.contains(&link_id), "pane.show should include link_id; got: {s}");

    // 4. Create a window + put the pane in it.
    let add_window = xscore()
        .args(["window", "add", "--cols", "120", "--rows", "30"])
        .output().unwrap();
    assert!(ok(&add_window));
    let window_id = parse_id_from_stdout(&add_window.stdout, "window");

    let add_to_window = xscore()
        .args(["pane", "add-to-window", "--pane", &pane_id, "--window", &window_id])
        .output().unwrap();
    assert!(ok(&add_to_window),
        "add-to-window failed: {}", String::from_utf8_lossy(&add_to_window.stderr));

    // 5. Verify the lists agree.
    let window_list = xscore().args(["window", "list"]).output().unwrap();
    let wl = String::from_utf8_lossy(&window_list.stdout);
    assert!(wl.contains(&window_id), "window list should contain wid: {wl}");

    let pane_list = xscore()
        .args(["pane", "list", "--window", &window_id])
        .output().unwrap();
    let pl = String::from_utf8_lossy(&pane_list.stdout);
    assert!(pl.contains(&pane_id),
        "pane list (--window) should contain pane id; got: {pl}");

    // 6. Detach the pane (orphan again).
    let detach = xscore()
        .args(["pane", "detach", "--pane", &pane_id])
        .output().unwrap();
    assert!(ok(&detach));

    // 7. data write fails now (orphan).
    let write_fail = xscore()
        .args(["data", "write", "--pane", &pane_id])
        .stdin(std::process::Stdio::piped())
        .output().unwrap();
    assert!(!write_fail.status.success(),
        "data_write on detached pane must fail");
    let s = String::from_utf8_lossy(&write_fail.stderr);
    assert!(s.contains("not attached") || s.contains("NotAttached"),
        "expected NotAttached error, got: {s}");

    // Cleanup
    let _ = xscore().args(["link", "del", "--id", &link_id]).output();
    let _ = xscore().args(["window", "del", "--id", &window_id]).output();
    let _ = xscore().args(["pane", "del", "--id", &pane_id]).output();
    let _ = xscore().args(["pane", "del", "--id", &auto_pane]).output();
    cleanup_daemon("full_flow");
}

// ----- Q1: direct link 1-active-pane auto-detach -----

#[test]
fn xscore_q1_direct_link_auto_detaches_old_active_pane() {
    let xscore = || xscore_for("q1_auto_detach");
    let add = xscore().args(["link", "add", "shell:///bin/cat"]).output().unwrap();
    let link_id = parse_id_from_stdout(&add.stdout, "link");

    let p1 = parse_id_from_stdout(&xscore().args(["pane", "add"]).output().unwrap().stdout, "pane");
    let p2 = parse_id_from_stdout(&xscore().args(["pane", "add"]).output().unwrap().stdout, "pane");

    let _ = xscore().args(["pane", "attach", "--pane", &p1, "--link", &link_id]).output().unwrap();
    let _ = xscore().args(["pane", "attach", "--pane", &p2, "--link", &link_id]).output().unwrap();

    // p1 should be detached (link_id: null); p2 should be attached.
    let show_p1 = xscore().args(["pane", "show", "--id", &p1]).output().unwrap();
    let p1_json = String::from_utf8_lossy(&show_p1.stdout);
    assert!(
        p1_json.contains(r#""link_id": null"#) || p1_json.contains(r#""link_id":null"#),
        "p1 should be detached, got: {p1_json}"
    );

    let show_p2 = xscore().args(["pane", "show", "--id", &p2]).output().unwrap();
    let p2_json = String::from_utf8_lossy(&show_p2.stdout);
    assert!(p2_json.contains(&link_id),
        "p2 should be attached, got: {p2_json}");

    let _ = xscore().args(["link", "del", "--id", &link_id]).output();
    let _ = xscore().args(["pane", "del", "--id", &p1]).output();
    let _ = xscore().args(["pane", "del", "--id", &p2]).output();
    cleanup_daemon("q1_auto_detach");
}

// ----- window add/list/del without a link -----

#[test]
fn xscore_window_add_list_del_orphan() {
    let xscore = || xscore_for("window_orphan");
    let add = xscore()
        .args(["window", "add", "--name", "scratch", "--cols", "100", "--rows", "20"])
        .output().unwrap();
    assert!(ok(&add));
    let wid = parse_id_from_stdout(&add.stdout, "window");

    let list = xscore().args(["window", "list"]).output().unwrap();
    let ls = String::from_utf8_lossy(&list.stdout);
    assert!(ls.contains(&wid), "window list should contain wid: {ls}");

    let del = xscore().args(["window", "del", "--id", &wid]).output().unwrap();
    assert!(ok(&del));
}

// ----- link del also detaches panes (panes survive as orphans) -----

#[test]
fn xscore_link_del_detaches_pane_keeps_pane_alive() {
    let xscore = || xscore_for("link_del_detaches");
    let add = xscore().args(["link", "add", "shell:///bin/cat"]).output().unwrap();
    let link_id = parse_id_from_stdout(&add.stdout, "link");
    let add_pane = xscore().args(["pane", "add"]).output().unwrap();
    let pane_id = parse_id_from_stdout(&add_pane.stdout, "pane");
    let _ = xscore()
        .args(["pane", "attach", "--pane", &pane_id, "--link", &link_id])
        .output().unwrap();

    let del = xscore().args(["link", "del", "--id", &link_id]).output().unwrap();
    assert!(ok(&del));

    // The pane survives — info.link_id is now None.
    let show = xscore().args(["pane", "show", "--id", &pane_id]).output().unwrap();
    let s = String::from_utf8_lossy(&show.stdout);
    assert!(
        s.contains(r#""link_id": null"#) || s.contains(r#""link_id":null"#),
        "pane.link_id should be null after link del; got: {s}"
    );

    let _ = xscore().args(["pane", "del", "--id", &pane_id]).output();
    cleanup_daemon("link_del_detaches");
}

// ----- v0.5.0 (ADR 0009): binding AND connection survive a restart -----
//
// v0.4.1 asserted the opposite (`connected: false`, write fails): the
// transport lived in the process that created it, so every later
// invocation saw a dead link. v0.5.0 moves the transport into the
// resident daemon, so a *later process* must now find the link alive and
// writable. This test is the inverse of the old one on purpose.

#[test]
fn xscore_direct_link_stays_connected_after_restart() {
    let xscore = || xscore_for("direct_stays_connected");
    cleanup_daemon("direct_stays_connected");

    // Process 1: create a direct link. It auto-creates a window + pane.
    let add = xscore().args(["link", "add", "shell:///bin/cat"]).output().unwrap();
    assert!(ok(&add));
    let link_id = parse_id_from_stdout(&add.stdout, "link");
    let pane_id = parse_id_from_stdout(&add.stdout, "pane");

    // Process 2: the transport did NOT die with process 1 — the daemon
    // holds it, so `link show` must report a live connection.
    let show = xscore().args(["link", "show", "--id", &link_id]).output().unwrap();
    assert!(ok(&show));
    let link = json(&show.stdout);
    assert_eq!(link["connected"], serde_json::json!(true),
        "direct link must stay connected across processes: {link}");

    // The pane's binding is still real.
    let pane = json(&xscore()
        .args(["pane", "show", "--id", &pane_id])
        .output().unwrap().stdout);
    assert_eq!(pane["link_id"], serde_json::json!(link_id),
        "binding must survive the restart: {pane}");

    // Process 3: and I/O actually works from a process that never had the
    // transport. This is the whole point of v0.5.0.
    let mut child = xscore()
        .args(["data", "write", "--pane", &pane_id])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    {
        use std::io::Write;
        child.stdin.as_mut().unwrap().write_all(b"still-here\n").unwrap();
    }
    let out = child.wait_with_output().unwrap();
    assert!(out.status.success(),
        "write on a daemon-backed link must succeed: {}",
        String::from_utf8_lossy(&out.stderr));

    let _ = xscore().args(["link", "del", "--id", &link_id]).output();
    let _ = xscore().args(["pane", "del", "--id", &pane_id]).output();
    cleanup_daemon("direct_stays_connected");
}

// ===== v0.5.0: the resident daemon (ADR 0009) =====

/// Test 1 — the reason the daemon exists.
///
/// Three separate processes: one creates the link, one writes, one reads.
/// Before v0.5.0 step 2 was impossible by construction (the transport died
/// with the creating process), so this test is the real acceptance test
/// for the feature.
#[test]
fn xscore_daemon_direct_link_survives_process() {
    let name = "daemon_survives";
    let xscore = || xscore_for(name);
    cleanup_daemon(name);

    // Process A: create the link.
    let add = xscore().args(["link", "add", "shell:///bin/cat"]).output().unwrap();
    assert!(ok(&add), "link add failed: {}", String::from_utf8_lossy(&add.stderr));
    let link_id = parse_id_from_stdout(&add.stdout, "link");
    let pane_id = parse_id_from_stdout(&add.stdout, "pane");

    // Process B: write into the link the daemon is holding.
    let mut child = xscore()
        .args(["data", "write", "--pane", &pane_id])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    {
        use std::io::Write;
        child.stdin.as_mut().unwrap().write_all(b"hello\n").unwrap();
    }
    let out = child.wait_with_output().unwrap();
    assert!(out.status.success(),
        "cross-process write failed: {}", String::from_utf8_lossy(&out.stderr));

    // `/bin/cat` echoes; give the daemon's pump time to drain it.
    std::thread::sleep(std::time::Duration::from_millis(300));

    // Process C: read it back.
    let read = xscore().args(["data", "read", "--pane", &pane_id]).output().unwrap();
    assert!(ok(&read), "cross-process read failed: {}", String::from_utf8_lossy(&read.stderr));
    let s = String::from_utf8_lossy(&read.stdout);
    assert!(s.contains("hello"),
        "read from a third process must see the daemon's buffer, got: {s:?}");

    let _ = xscore().args(["link", "del", "--id", &link_id]).output();
    let _ = xscore().args(["pane", "del", "--id", &pane_id]).output();
    cleanup_daemon(name);
}

/// Test 2 — `daemon status` names the link (and its pane) it is holding.
#[test]
fn xscore_daemon_status_lists_link() {
    let name = "daemon_status";
    let xscore = || xscore_for(name);
    cleanup_daemon(name);

    let add = xscore().args(["link", "add", "shell:///bin/cat"]).output().unwrap();
    assert!(ok(&add));
    let link_id = parse_id_from_stdout(&add.stdout, "link");
    let pane_id = parse_id_from_stdout(&add.stdout, "pane");

    let status = xscore().args(["daemon", "status"]).output().unwrap();
    assert!(ok(&status), "daemon status failed: {}", String::from_utf8_lossy(&status.stderr));
    let v = json(&status.stdout);
    assert_eq!(v["ok"], serde_json::json!(true), "{v}");
    let links = v["status"]["links"].as_array().expect("links array");
    assert!(links.iter().any(|l| l == &serde_json::json!(link_id)),
        "status must list {link_id}: {v}");
    let panes = v["status"]["panes"].as_array().expect("panes array");
    let pane = panes.iter().find(|p| p["pane"] == serde_json::json!(pane_id))
        .unwrap_or_else(|| panic!("status must list pane {pane_id}: {v}"));
    assert_eq!(pane["link"], serde_json::json!(link_id),
        "pane must be reported as attached: {pane}");
    assert!(v["status"]["pid"].as_u64().unwrap() > 0, "pid missing: {v}");

    let _ = xscore().args(["link", "del", "--id", &link_id]).output();
    let _ = xscore().args(["pane", "del", "--id", &pane_id]).output();
    cleanup_daemon(name);
}

/// Test 3 — `link show`'s `connected` is a live daemon probe.
#[test]
fn xscore_daemon_link_show_connected_true() {
    let name = "daemon_connected";
    let xscore = || xscore_for(name);
    cleanup_daemon(name);

    let add = xscore().args(["link", "add", "shell:///bin/cat"]).output().unwrap();
    assert!(ok(&add));
    let link_id = parse_id_from_stdout(&add.stdout, "link");
    let pane_id = parse_id_from_stdout(&add.stdout, "pane");

    let show = xscore().args(["link", "show", "--id", &link_id]).output().unwrap();
    assert!(ok(&show));
    let link = json(&show.stdout);
    assert_eq!(link["connected"], serde_json::json!(true),
        "a daemon-held link is connected: {link}");

    let _ = xscore().args(["link", "del", "--id", &link_id]).output();
    let _ = xscore().args(["pane", "del", "--id", &pane_id]).output();
    cleanup_daemon(name);
}

/// Test 4 — stopping the daemon really does disconnect the link, and data
/// ops say `LinkNotConnected` (not "you never attached anything").
#[test]
fn xscore_daemon_stopped_link_not_connected() {
    let name = "daemon_stopped";
    let xscore = || xscore_for(name);
    cleanup_daemon(name);

    let add = xscore().args(["link", "add", "shell:///bin/cat"]).output().unwrap();
    assert!(ok(&add));
    let link_id = parse_id_from_stdout(&add.stdout, "link");
    let pane_id = parse_id_from_stdout(&add.stdout, "pane");

    let stop = xscore().args(["daemon", "stop"]).output().unwrap();
    assert!(ok(&stop), "daemon stop failed: {}", String::from_utf8_lossy(&stop.stderr));

    // The metadata is untouched; only the connection went away.
    let show = xscore().args(["link", "show", "--id", &link_id]).output().unwrap();
    assert!(ok(&show));
    let link = json(&show.stdout);
    assert_eq!(link["connected"], serde_json::json!(false),
        "stopped daemon ⇒ connected=false: {link}");

    let pane = json(&xscore()
        .args(["pane", "show", "--id", &pane_id])
        .output().unwrap().stdout);
    assert_eq!(pane["link_id"], serde_json::json!(link_id),
        "the binding must survive `daemon stop`: {pane}");

    let write = xscore()
        .args(["data", "write", "--pane", &pane_id])
        .output().unwrap();
    assert!(!write.status.success(), "write to a stopped daemon must fail");
    let err = String::from_utf8_lossy(&write.stderr);
    assert!(
        err.contains("not connected") || err.contains("LinkNotConnected"),
        "expected LinkNotConnected, got: {err}"
    );
    assert!(!err.contains("not attached"),
        "must not report NotAttached for a bound pane: {err}");

    let _ = xscore().args(["link", "del", "--id", &link_id]).output();
    let _ = xscore().args(["pane", "del", "--id", &pane_id]).output();
    cleanup_daemon(name);
}

/// Test 5 — a pane's buffer belongs to the daemon, so detaching in one
/// process leaves the history readable in the next, and a re-attach picks
/// up where it left off.
#[test]
fn xscore_daemon_buffer_survives_detach_across_processes() {
    let name = "daemon_detach";
    let xscore = || xscore_for(name);
    cleanup_daemon(name);

    // Process A: link + write.
    let add = xscore().args(["link", "add", "shell:///bin/cat"]).output().unwrap();
    assert!(ok(&add));
    let link_id = parse_id_from_stdout(&add.stdout, "link");
    let pane_id = parse_id_from_stdout(&add.stdout, "pane");

    let mut child = xscore()
        .args(["data", "write", "--pane", &pane_id])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    {
        use std::io::Write;
        child.stdin.as_mut().unwrap().write_all(b"kept\n").unwrap();
    }
    assert!(child.wait_with_output().unwrap().status.success());

    // Let the pump drain cat's echo BEFORE we stop the pump.
    std::thread::sleep(std::time::Duration::from_millis(250));

    // Process B: detach.
    let detach = xscore().args(["pane", "detach", "--pane", &pane_id]).output().unwrap();
    assert!(ok(&detach), "detach failed: {}", String::from_utf8_lossy(&detach.stderr));
    std::thread::sleep(std::time::Duration::from_millis(100));

    // Process C: the buffer is still there (a pane is its buffer), even
    // though the pane is orphaned.
    let read = xscore().args(["data", "read", "--pane", &pane_id]).output().unwrap();
    assert!(ok(&read), "read after detach failed: {}", String::from_utf8_lossy(&read.stderr));
    let s = String::from_utf8_lossy(&read.stdout);
    assert!(s.contains("kept"),
        "detached pane must keep its buffer across processes, got: {s:?}");

    // Writing is refused while detached (no link to write to) ...
    let refused = xscore().args(["data", "write", "--pane", &pane_id]).output().unwrap();
    assert!(!refused.status.success(), "write on a detached pane must fail");

    // ... and re-attaching makes the pane writable again, into the SAME
    // buffer: the old bytes are still ahead of the new ones.
    let attach = xscore()
        .args(["pane", "attach", "--pane", &pane_id, "--link", &link_id])
        .output().unwrap();
    assert!(ok(&attach), "re-attach failed: {}", String::from_utf8_lossy(&attach.stderr));

    let mut child = xscore()
        .args(["data", "write", "--pane", &pane_id])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    {
        use std::io::Write;
        child.stdin.as_mut().unwrap().write_all(b"more\n").unwrap();
    }
    assert!(child.wait_with_output().unwrap().status.success());
    std::thread::sleep(std::time::Duration::from_millis(300));

    let read = xscore().args(["data", "read", "--pane", &pane_id]).output().unwrap();
    assert!(ok(&read));
    let s = String::from_utf8_lossy(&read.stdout);
    assert!(s.contains("kept") && s.contains("more"),
        "re-attach must append to the same buffer, got: {s:?}");

    let _ = xscore().args(["link", "del", "--id", &link_id]).output();
    let _ = xscore().args(["pane", "del", "--id", &pane_id]).output();
    cleanup_daemon(name);
}

// ----- v0.5.1: the daemon must not die when its starting terminal goes away -----
//
// The daemon is auto-started from whatever command first needed it. If it
// inherited that command's session, closing the terminal (SIGHUP) or
// Ctrl-C (SIGINT) would take every live direct link down with it, and any
// wrapper waiting on the caller's process group would block until the
// daemon exits — which it never does while a link is alive.
//
// Two things keep it safe and both are asserted here: the daemon runs in
// its own session (`setsid` at spawn) and it ignores SIGHUP.

#[test]
fn xscore_daemon_survives_sighup() {
    let name = "daemon_sighup";
    let xscore = || xscore_for(name);
    cleanup_daemon(name);

    let add = xscore().args(["link", "add", "shell:///bin/cat"]).output().unwrap();
    assert!(ok(&add), "add failed: {}", String::from_utf8_lossy(&add.stderr));
    let link_id = parse_id_from_stdout(&add.stdout, "link");
    let pane_id = parse_id_from_stdout(&add.stdout, "pane");

    let status = json(&xscore().args(["daemon", "status"]).output().unwrap().stdout);
    let pid = status["status"]["pid"].as_u64().expect("daemon pid");

    // Hang up on the daemon the way a closing terminal would.
    let killed = std::process::Command::new("kill")
        .args(["-HUP", &pid.to_string()])
        .status()
        .unwrap();
    assert!(killed.success(), "failed to send SIGHUP to {pid}");
    std::thread::sleep(std::time::Duration::from_millis(500));

    // Still serving: the pid is unchanged and the link is still connected.
    let after = json(&xscore().args(["daemon", "status"]).output().unwrap().stdout);
    assert_eq!(after["status"]["pid"], serde_json::json!(pid),
        "daemon must survive SIGHUP: {after}");
    let link = json(&xscore().args(["link", "show", "--id", &link_id]).output()
        .unwrap().stdout);
    assert_eq!(link["connected"], serde_json::json!(true),
        "link must stay connected after SIGHUP: {link}");

    let _ = xscore().args(["link", "del", "--id", &link_id]).output();
    let _ = xscore().args(["pane", "del", "--id", &pane_id]).output();
    cleanup_daemon(name);
}

// ----- v0.6.0 (ADR 0010): the session is the workspace -----
//
// A session owns links, windows and panes; every child carries `session_id`.
// `session new` is the one-command path (first link + window + pane), `kill`
// is the only cascade, and `forget` only applies to tmux-backed sessions.

#[test]
fn xscore_session_new_creates_window_link_and_pane() {
    let name = "session_new_creates";
    let xscore = || xscore_for(name);

    let out = xscore()
        .args(["session", "new", "work"])
        .output()
        .unwrap();
    assert!(ok(&out), "session new failed: {}", String::from_utf8_lossy(&out.stderr));

    let sid = parse_id_from_stdout(&out.stdout, "session");
    let lid = parse_id_from_stdout(&out.stdout, "link");
    let wid = parse_id_from_stdout(&out.stdout, "window");
    let pid = parse_id_from_stdout(&out.stdout, "pane");

    // Every child points back at the session it was created in.
    let pane = json(&xscore().args(["pane", "show", "--id", &pid]).output().unwrap().stdout);
    assert_eq!(pane["session_id"], serde_json::json!(sid), "pane session: {pane}");
    assert_eq!(pane["link_id"], serde_json::json!(lid), "pane link: {pane}");
    assert_eq!(pane["window_id"], serde_json::json!(wid), "pane window: {pane}");

    let window = json(&xscore().args(["window", "show", "--id", &wid]).output().unwrap().stdout);
    assert_eq!(window["session_id"], serde_json::json!(sid), "window session: {window}");

    let link = json(&xscore().args(["link", "show", "--id", &lid]).output().unwrap().stdout);
    assert_eq!(link["session_id"], serde_json::json!(sid), "link session: {link}");

    cleanup_daemon(name);
}

#[test]
fn xscore_session_ls_lists_it() {
    let name = "session_ls";
    let xscore = || xscore_for(name);

    let new = xscore()
        .args(["session", "new", "work", "--empty"])
        .output()
        .unwrap();
    assert!(ok(&new), "session new --empty failed: {}", String::from_utf8_lossy(&new.stderr));
    let sid = parse_id_from_stdout(&new.stdout, "session");

    let list = xscore().args(["session", "ls"]).output().unwrap();
    assert!(ok(&list));
    let s = String::from_utf8_lossy(&list.stdout);
    assert!(s.contains("SESSION ID"), "missing header: {s}");
    assert!(s.contains(&sid), "missing session id: {s}");
    assert!(s.contains("work"), "missing session name: {s}");
    assert!(s.contains("native"), "missing backend: {s}");
}

#[test]
fn xscore_session_kill_cascades() {
    let name = "session_kill_cascades";
    let xscore = || xscore_for(name);

    let new = xscore().args(["session", "new", "work"]).output().unwrap();
    assert!(ok(&new), "session new failed: {}", String::from_utf8_lossy(&new.stderr));

    let kill = xscore().args(["session", "kill", "work"]).output().unwrap();
    assert!(ok(&kill), "session kill failed: {}", String::from_utf8_lossy(&kill.stderr));

    // No sessions left...
    let sessions = xscore().args(["session", "ls"]).output().unwrap();
    let s = String::from_utf8_lossy(&sessions.stdout);
    assert!(!s.contains("work"), "session still listed: {s}");
    assert_eq!(s.lines().count(), 1, "only the header should remain: {s}");

    // ...and nothing that belonged to it.
    let links = xscore().args(["link", "list"]).output().unwrap();
    let s = String::from_utf8_lossy(&links.stdout);
    assert_eq!(s.lines().count(), 1, "link list must be empty after kill: {s}");

    cleanup_daemon(name);
}

#[test]
fn xscore_session_forget_native_is_refused() {
    let name = "session_forget_native";
    let xscore = || xscore_for(name);

    let new = xscore()
        .args(["session", "new", "work", "--empty"])
        .output()
        .unwrap();
    assert!(ok(&new), "session new failed: {}", String::from_utf8_lossy(&new.stderr));

    let forget = xscore().args(["session", "forget", "work"]).output().unwrap();
    assert!(
        !forget.status.success(),
        "`session forget` on a native session must be refused"
    );
    let err = String::from_utf8_lossy(&forget.stderr);
    assert!(err.contains("native"), "got: {err}");
    assert!(err.contains("session kill"), "error should name the right verb: {err}");

    // Refused means untouched: the session is still there.
    let sessions = xscore().args(["session", "ls"]).output().unwrap();
    assert!(
        String::from_utf8_lossy(&sessions.stdout).contains("work"),
        "a refused forget must not drop the session"
    );
}

#[test]
fn xscore_link_add_with_session_flag() {
    let name = "link_add_with_session";
    let xscore = || xscore_for(name);

    let new = xscore()
        .args(["session", "new", "A", "--empty"])
        .output()
        .unwrap();
    assert!(ok(&new), "session new failed: {}", String::from_utf8_lossy(&new.stderr));
    let sid = parse_id_from_stdout(&new.stdout, "session");

    let add = xscore()
        .args(["link", "add", "shell:///bin/cat", "--session", "A"])
        .output()
        .unwrap();
    assert!(ok(&add), "link add --session A failed: {}", String::from_utf8_lossy(&add.stderr));
    let lid = parse_id_from_stdout(&add.stdout, "link");
    let pid = parse_id_from_stdout(&add.stdout, "pane");

    let pane = json(&xscore().args(["pane", "show", "--id", &pid]).output().unwrap().stdout);
    assert_eq!(pane["session_id"], serde_json::json!(sid), "pane joined A: {pane}");

    let show = xscore().args(["session", "show", "A"]).output().unwrap();
    assert!(ok(&show), "session show failed: {}", String::from_utf8_lossy(&show.stderr));
    let s = String::from_utf8_lossy(&show.stdout);
    assert!(s.contains(&lid), "session show A must list the link {lid}: {s}");
    assert!(s.contains(&pid), "session show A must list the pane {pid}: {s}");

    // A link in another session is not visible here.
    let other = xscore()
        .args(["session", "new", "B", "--empty"])
        .output()
        .unwrap();
    assert!(ok(&other));
    let show_a = xscore().args(["session", "show", "A"]).output().unwrap();
    assert!(String::from_utf8_lossy(&show_a.stdout).contains(&lid));

    cleanup_daemon(name);
}

// ----- v0.6.0 (PR3b, ADR 0010 §D2): tmux-backed sessions over control mode -----
//
// These drive a real tmux server on the test's own socket (`XSCORE_SOCKET`,
// set by `xscore_bin`), and assert what TMUX has after the command, not what
// xscore says it did — the two disagreeing is the bug this layer can hide.
//
// Note that every `xscore` call is a separate process: there is no client
// attached between commands, so a command that needs live structure attaches
// for its own lifetime. That is the CLI's actual shape in v0.6 (ADR 0009's
// daemon is what will keep the client instead).

/// This test's tmux server.
fn tmux_for(test_name: &str) -> Command {
    let mut cmd = Command::new("tmux");
    cmd.args(["-L", &format!("xscore-test-{test_name}")]);
    cmd
}

/// Run one tmux command against this test's server and return its stdout.
fn tmux_stdout(test_name: &str, args: &[&str]) -> String {
    let out = tmux_for(test_name)
        .args(args)
        .output()
        .unwrap_or_else(|e| panic!("run tmux {args:?}: {e}"));
    String::from_utf8_lossy(&out.stdout).to_string()
}

#[test]
fn xscore_session_new_tmux_creates_window_and_pane() {
    let name = "session_new_tmux";
    let xscore = || xscore_for(name);
    cleanup_tmux(name);

    let out = xscore()
        .args(["session", "new", "work", "--tmux", "--cols", "100", "--rows", "30"])
        .output()
        .unwrap();
    assert!(ok(&out), "session new --tmux failed: {}", String::from_utf8_lossy(&out.stderr));

    let sid = parse_id_from_stdout(&out.stdout, "session");
    let lid = parse_id_from_stdout(&out.stdout, "link");
    let wid = parse_id_from_stdout(&out.stdout, "window");
    let pid = parse_id_from_stdout(&out.stdout, "pane");

    // A real tmux session, named after the xscore session, with one window.
    let windows = tmux_stdout(name, &["list-windows", "-t", "work", "-F", "#{window_id}"]);
    assert_eq!(windows.lines().count(), 1, "expected one tmux window: {windows:?}");

    // The pane row is a `%N` on the session's control link — not a sink handle.
    let pane = json(&xscore().args(["pane", "show", "--id", &pid]).output().unwrap().stdout);
    assert_eq!(pane["session_id"], serde_json::json!(sid));
    assert_eq!(pane["link_id"], serde_json::json!(lid));
    let key = pane["link_key"].as_str().expect("link_key must hold the %N handle");
    assert!(key.starts_with('%'), "pane key should be %N, got {key}");
    assert!(pane["link_key"].is_string(), "control panes address by %N in link_key: {pane}");
    assert!(pane["cols"].as_u64().unwrap_or(0) > 0, "geometry adopted from tmux: {pane}");

    // The window mirrors `@N`.
    let window = json(&xscore().args(["window", "show", "--id", &wid]).output().unwrap().stdout);
    let handle = window["backend"]["handle"].as_str().expect("WindowBackend::Tmux handle");
    assert!(handle.starts_with('@'), "window handle should be @N, got {handle}");

    // `session show` names the backend and the tmux session.
    let show = String::from_utf8_lossy(
        &xscore().args(["session", "show", "work"]).output().unwrap().stdout,
    )
    .to_string();
    assert!(show.contains("backend: tmux"), "backend line missing: {show}");
    assert!(
        show.lines().any(|l| l.starts_with("tmux:") && l.contains("work")),
        "the tmux session name is missing: {show}"
    );

    // One tmux session, one link: a second `link add` into it is refused, and a
    // second `session new` of the same tmux name is refused too.
    let link = xscore()
        .args(["link", "add", "shell:///bin/cat", "--session", "work"])
        .output()
        .unwrap();
    assert!(!link.status.success(), "a tmux session must not take a second link");
    let again = xscore().args(["session", "new", "work", "--tmux"]).output().unwrap();
    assert!(!again.status.success(), "a duplicate tmux session must be refused");

    let _ = xscore().args(["session", "kill", "work"]).output();
    cleanup_tmux(name);
}

#[test]
fn xscore_session_new_tmux_data_roundtrip() {
    let name = "session_tmux_roundtrip";
    let xscore = || xscore_for(name);
    cleanup_tmux(name);

    // `cat` echoes what it reads, so a write that reached the pane comes back
    // as that pane's bytes.
    let out = xscore()
        .args(["session", "new", "work", "--tmux", "--cmd", "cat"])
        .output()
        .unwrap();
    assert!(ok(&out), "session new --tmux failed: {}", String::from_utf8_lossy(&out.stderr));
    let pid = parse_id_from_stdout(&out.stdout, "pane");

    let payload = home_for(name).join("roundtrip.txt");
    std::fs::write(&payload, b"hello\n").unwrap();

    let write = xscore()
        .args(["data", "write", "--pane", &pid, "--from-file", payload.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(ok(&write), "data write failed: {}", String::from_utf8_lossy(&write.stderr));

    // The read is a NEW process: it hydrates state, attaches its own control
    // client for the length of the command, and must still see the bytes.
    let mut seen = String::new();
    for _ in 0..50 {
        let read = xscore()
            .args(["data", "read", "--pane", &pid, "--bytes", "4096"])
            .output()
            .unwrap();
        assert!(ok(&read), "data read failed: {}", String::from_utf8_lossy(&read.stderr));
        seen = String::from_utf8_lossy(&read.stdout).to_string();
        if seen.contains("hello") {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    assert!(seen.contains("hello"), "data read did not return what was written: {seen:?}");

    let _ = xscore().args(["session", "kill", "work"]).output();
    cleanup_tmux(name);
}

#[test]
fn xscore_session_new_tmux_write_is_literal() {
    let name = "session_tmux_literal";
    let xscore = || xscore_for(name);
    cleanup_tmux(name);

    // `stty -echo` keeps the terminal's own echo off the screen, so the screen
    // shows exactly what the pane's `cat` received. Plain `cat`, not `cat -v`:
    // `-v` would re-encode the non-ASCII bytes we are checking.
    let out = xscore()
        .args([
            "session", "new", "work", "--tmux",
            "--cmd", "stty -echo; exec cat",
            "--cols", "120", "--rows", "10",
        ])
        .output()
        .unwrap();
    assert!(ok(&out), "session new --tmux failed: {}", String::from_utf8_lossy(&out.stderr));
    let pid = parse_id_from_stdout(&out.stdout, "pane");
    let pane = json(&xscore().args(["pane", "show", "--id", &pid]).output().unwrap().stdout);
    let key = pane["link_key"].as_str().expect("link_key").to_string();

    // Every character that has ever broken a quoting layer: a command
    // separator, a quote, a double quote, a variable, a backslash, a space, a
    // percent (tmux switches on it in formats) and multi-byte UTF-8.
    let payload = "a;b it's \"q\" $X \\ Z 中文 % #";
    let body = home_for(name).join("payload.txt");
    let newline = home_for(name).join("newline.txt");
    std::fs::write(&body, payload).unwrap();
    std::fs::write(&newline, "\n").unwrap();

    // Two writes on purpose. Without a newline the payload is one `send-keys -l`
    // line (the quoted form under test) and stays in the pane's line discipline;
    // the newline then flushes it. The second write is the `-H` hex form.
    for part in [&body, &newline] {
        let write = xscore()
            .args(["data", "write", "--pane", &pid, "--from-file", part.to_str().unwrap()])
            .output()
            .unwrap();
        assert!(ok(&write), "data write failed: {}", String::from_utf8_lossy(&write.stderr));
    }

    let mut screen = String::new();
    for _ in 0..50 {
        screen = tmux_stdout(name, &["capture-pane", "-p", "-t", &key]);
        if screen.lines().next() == Some(payload) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    assert_eq!(
        screen.lines().next(),
        Some(payload),
        "the pane received something other than the exact payload: {screen:?}"
    );

    let _ = xscore().args(["session", "kill", "work"]).output();
    cleanup_tmux(name);
}

#[test]
fn xscore_session_adopt_existing_tmux() {
    let name = "session_adopt_tmux";
    let xscore = || xscore_for(name);
    cleanup_tmux(name);

    // Created outside xscore, the way a user's session exists before adoption.
    let created = tmux_for(name)
        .args(["new-session", "-d", "-s", "outside", "-x", "90", "-y", "20"])
        .output()
        .unwrap();
    assert!(created.status.success(), "tmux new-session failed");

    let out = xscore().args(["session", "adopt", "outside"]).output().unwrap();
    assert!(ok(&out), "session adopt failed: {}", String::from_utf8_lossy(&out.stderr));
    let lid = parse_id_from_stdout(&out.stdout, "link");

    // One pane, bound to the control link the adoption created.
    let ls = xscore().args(["pane", "list", "--link", &lid]).output().unwrap();
    assert!(ok(&ls), "pane list failed: {}", String::from_utf8_lossy(&ls.stderr));
    let stdout = String::from_utf8_lossy(&ls.stdout).to_string();
    let rows = stdout.lines().skip(1).filter(|l| !l.trim().is_empty()).count();
    assert_eq!(rows, 1, "expected exactly one adopted pane: {stdout}");

    let show = String::from_utf8_lossy(
        &xscore().args(["session", "show", "outside"]).output().unwrap().stdout,
    )
    .to_string();
    assert!(show.contains("backend: tmux"), "backend line missing: {show}");
    assert!(show.contains("outside"), "tmux session name missing: {show}");

    // `forget` detaches without touching the server session (arch.md §8).
    let forget = xscore().args(["session", "forget", "outside"]).output().unwrap();
    assert!(ok(&forget), "session forget failed: {}", String::from_utf8_lossy(&forget.stderr));
    let alive = tmux_for(name).args(["has-session", "-t", "outside"]).output().unwrap();
    assert!(alive.status.success(), "`session forget` must leave tmux running");

    cleanup_tmux(name);
}

#[test]
fn xscore_session_kill_tmux_ends_the_server_session() {
    let name = "session_kill_tmux";
    let xscore = || xscore_for(name);
    cleanup_tmux(name);

    let out = xscore()
        .args(["session", "new", "work", "--tmux"])
        .output()
        .unwrap();
    assert!(ok(&out), "session new --tmux failed: {}", String::from_utf8_lossy(&out.stderr));

    let alive_before = tmux_for(name).args(["has-session", "-t", "work"]).output().unwrap();
    assert!(alive_before.status.success(), "the tmux session should exist before the kill");

    let kill = xscore().args(["session", "kill", "work"]).output().unwrap();
    assert!(ok(&kill), "session kill failed: {}", String::from_utf8_lossy(&kill.stderr));

    let alive_after = tmux_for(name).args(["has-session", "-t", "work"]).output().unwrap();
    assert!(
        !alive_after.status.success(),
        "the tmux session survived `session kill`"
    );

    // ...and the rows are gone too.
    let show = xscore().args(["session", "show", "work"]).output().unwrap();
    assert!(!show.status.success(), "a killed session must not resolve: {show:?}");

    cleanup_tmux(name);
}

#[test]
fn xscore_session_tmux_sees_an_externally_created_window() {
    let name = "session_tmux_external_window";
    let xscore = || xscore_for(name);
    cleanup_tmux(name);

    let out = xscore()
        .args(["session", "new", "work", "--tmux"])
        .output()
        .unwrap();
    assert!(ok(&out), "session new --tmux failed: {}", String::from_utf8_lossy(&out.stderr));

    // The structure change happens outside xscore, in a different process, at a
    // moment when nothing is attached to the session.
    let added = tmux_for(name)
        .args(["new-window", "-d", "-t", "work", "-n", "extra"])
        .output()
        .unwrap();
    assert!(added.status.success(), "tmux new-window failed");

    // `window list` attaches for its own lifetime, so it reports what tmux has
    // now. Poll: attaching and enumerating is not instantaneous.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    let mut rows = 0;
    let mut stdout = String::new();
    while std::time::Instant::now() < deadline {
        let list = xscore().args(["window", "list"]).output().unwrap();
        assert!(ok(&list), "window list failed: {}", String::from_utf8_lossy(&list.stderr));
        stdout = String::from_utf8_lossy(&list.stdout).to_string();
        rows = stdout.lines().skip(1).filter(|l| !l.trim().is_empty()).count();
        if rows >= 2 && stdout.contains("extra") {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    assert_eq!(rows, 2, "the externally created window is missing: {stdout}");
    assert!(stdout.contains("extra"), "the window name was not adopted: {stdout}");

    let _ = xscore().args(["session", "kill", "work"]).output();
    cleanup_tmux(name);
}

// ----- v0.6.0 PR4: migrating a v3 file whose tmux session used the old path --
//
// This is the whole point of `migrate_v3_tmux_links`: the user's tmux session
// survives the mechanism change. The rows that described the old per-command
// path are dropped, and the first command that needs the session attaches a
// control client, which re-enumerates it.

/// A v3 `state.json` as the pre-PR4 build wrote it: one tmux session driven by
/// the old per-command tmux link, with one window and one pane.
fn legacy_v3_state(socket: &str) -> String {
    format!(
        r#"{{
  "schema_version": 3,
  "sessions": [
    {{"id": "01928e0a-7d10-7000-8000-000000000101", "name": "build",
      "created_at": "2026-10-01T10:00:00Z",
      "backend": {{"kind": "tmux", "host": {{"kind": "local"}},
                 "socket": {{"kind": "named", "name": "{socket}"}}, "session": "build"}}}}
  ],
  "links": [
    {{"id": "01928e0a-7d10-7000-8000-000000000201",
      "session_id": "01928e0a-7d10-7000-8000-000000000101",
      "name": "build", "created_at": "2026-10-01T10:00:00Z",
      "address": {{"transport": "shell", "command": "/bin/cat", "args": []}},
      "transport": {{"transport": "shell", "command": "/bin/cat", "args": []}},
      "sink": {{"sink": "tmux", "mode": "auto", "session_name": "build"}},
      "endpoint": "pipe"}}
  ],
  "windows": [
    {{"id": "01928e0a-7d10-7000-8000-000000000301",
      "session_id": "01928e0a-7d10-7000-8000-000000000101",
      "name": "sh", "created_at": "2026-10-01T10:00:00Z",
      "layout": {{"kind": "single"}}, "cols": 100, "rows": 30,
      "tmux_window_handle": "@0", "panes": ["01928e0a-7d10-7000-8000-000000000401"]}}
  ],
  "panes": [
    {{"id": "01928e0a-7d10-7000-8000-000000000401",
      "session_id": "01928e0a-7d10-7000-8000-000000000101",
      "name": "b1", "created_at": "2026-10-01T10:00:00Z",
      "cols": 100, "rows": 30, "x": 0, "y": 0,
      "link_id": "01928e0a-7d10-7000-8000-000000000201",
      "window_id": "01928e0a-7d10-7000-8000-000000000301",
      "sink_handle": "%0", "link_key": null}}
  ]
}}"#
    )
}

#[test]
fn xscore_state_migrate_rebuilds_a_tmux_session_over_control_mode() {
    let name = "state_migrate_tmux";
    let xscore = || xscore_for(name);
    cleanup_tmux(name);

    // The tmux session is the user's, and it already exists.
    let created = tmux_for(name)
        .args(["new-session", "-d", "-s", "build", "-x", "100", "-y", "30"])
        .output()
        .unwrap();
    assert!(created.status.success(), "tmux new-session failed");

    // First touch wipes this test's HOME, so the legacy file is written after.
    assert!(xscore().args(["session", "ls"]).output().unwrap().status.success());
    let state = home_for(name)
        .join(".config")
        .join("xscore")
        .join("state")
        .join("state.json");
    std::fs::create_dir_all(state.parent().unwrap()).unwrap();
    std::fs::write(&state, legacy_v3_state(&format!("xscore-test-{name}"))).unwrap();

    // Loading rows that still carry the old tmux path is refused, not silently
    // emptied: losing a tmux session without saying so is the failure this
    // test exists to prevent.
    let refused = xscore().args(["session", "ls"]).output().unwrap();
    assert!(!refused.status.success(), "an un-migrated v3 file must not load");
    let err = String::from_utf8_lossy(&refused.stderr);
    assert!(err.contains("state migrate"), "error should name the migration: {err}");

    let migrate = xscore().args(["state", "migrate"]).output().unwrap();
    assert!(ok(&migrate), "state migrate failed: {}", String::from_utf8_lossy(&migrate.stderr));
    let out = String::from_utf8_lossy(&migrate.stdout);
    assert!(out.contains("1 tmux session(s) converted"), "got: {out}");

    // The session survived, and the next command that needs its structure
    // attaches a control client — which rebuilds the rows from tmux itself.
    let windows = xscore().args(["window", "list", "--session", "build"]).output().unwrap();
    assert!(ok(&windows), "window list failed: {}", String::from_utf8_lossy(&windows.stderr));
    let wl = String::from_utf8_lossy(&windows.stdout).to_string();
    let rows = wl.lines().skip(1).filter(|l| !l.trim().is_empty()).count();
    assert_eq!(rows, 1, "the tmux window was not re-enumerated: {wl}");
    assert!(wl.contains('@'), "the window row must mirror @N: {wl}");

    let panes = xscore().args(["pane", "list", "--session", "build"]).output().unwrap();
    assert!(ok(&panes), "pane list failed: {}", String::from_utf8_lossy(&panes.stderr));
    let pl = String::from_utf8_lossy(&panes.stdout).to_string();
    assert_eq!(
        pl.lines().skip(1).filter(|l| !l.trim().is_empty()).count(),
        1,
        "the tmux pane was not re-enumerated: {pl}"
    );
    let pane_id = pl
        .lines()
        .nth(1)
        .and_then(|l| l.split_whitespace().next())
        .expect("a pane row");
    let pane = json(&xscore().args(["pane", "show", "--id", pane_id]).output().unwrap().stdout);
    assert!(
        pane["link_key"].as_str().map(|k| k.starts_with('%')).unwrap_or(false),
        "the re-enumerated pane is addressed by %N: {pane}"
    );

    // ...and the one link is the control client, created on that attach.
    let links = xscore().args(["link", "list", "--session", "build"]).output().unwrap();
    let ll = String::from_utf8_lossy(&links.stdout).to_string();
    assert_eq!(
        ll.lines().skip(1).filter(|l| !l.trim().is_empty()).count(),
        1,
        "one tmux session, one link: {ll}"
    );
    assert!(ll.contains("control"), "and it is the control link: {ll}");

    let _ = xscore().args(["session", "kill", "build"]).output();
    cleanup_tmux(name);
}

// ===== v0.6.0 PR5: the daemon owns the tmux control clients =====
//
// These are the acceptance tests for "a pane's data is a stream, not a screen".
// Every step is its own CLI process, so the only thing that can carry bytes
// from the writer to the reader is the daemon's buffer: if the client (and its
// `%output %N` subscription) lived in the CLI, the reader would have nothing
// and the old `capture-pane` fallback would answer with the rendered screen
// instead.
//
// The panes run `stty -echo raw; exec cat`:
//   * `-echo` keeps the terminal's own echo off the stream (the bytes below
//     must be what `cat` wrote, not what the line discipline showed);
//   * `raw` is what makes a write **without a newline** reach `cat` at all —
//     in cooked mode the pty holds it until a newline, so neither the screen
//     nor `%output` ever sees it and no implementation could read it.
//   * `cat` echoes what it reads back to its stdout, i.e. into the pane.

/// `data write --from-file` in its own process.
fn write_pane(xscore: &dyn Fn() -> Command, pane: &str, path: &std::path::Path) {
    let out = xscore()
        .args(["data", "write", "--pane", pane, "--from-file", path.to_str().unwrap()])
        .output()
        .expect("run data write");
    assert!(ok(&out), "data write failed: {}", String::from_utf8_lossy(&out.stderr));
}

/// `data read` in a fresh process, polled until it satisfies `done` (the pane's
/// pump and the process hand-off are not instantaneous). Returns the last read.
fn read_pane_until(
    xscore: &dyn Fn() -> Command,
    pane: &str,
    done: impl Fn(&str) -> bool,
) -> String {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    let mut last = String::new();
    while std::time::Instant::now() < deadline {
        let read = xscore()
            .args(["data", "read", "--pane", pane, "--bytes", "4096"])
            .output()
            .expect("run data read");
        assert!(ok(&read), "data read failed: {}", String::from_utf8_lossy(&read.stderr));
        last = String::from_utf8_lossy(&read.stdout).to_string();
        if done(&last) {
            return last;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    last
}

/// Test 1 — the discriminator between "stream" and "screen snapshot".
///
/// Process A writes three bytes with no newline; process B must read exactly
/// those three bytes back. Then a burst longer than the pane's screen proves
/// the buffer kept what the screen scrolled away.
#[test]
fn xscore_tmux_data_read_across_processes_is_a_stream() {
    let name = "tmux_stream";
    let xscore = || xscore_for(name);
    cleanup_tmux(name);
    cleanup_daemon(name);

    let created = xscore()
        .args([
            "session", "new", "work", "--tmux",
            "--cmd", "stty -echo raw; exec cat",
            "--cols", "40", "--rows", "3",
        ])
        .output()
        .unwrap();
    assert!(ok(&created), "session new --tmux failed: {}", String::from_utf8_lossy(&created.stderr));
    let pid = parse_id_from_stdout(&created.stdout, "pane");

    // Process A: three bytes, deliberately unterminated.
    let xyz = home_for(name).join("xyz.txt");
    std::fs::write(&xyz, b"XYZ").unwrap();
    write_pane(&xscore, &pid, &xyz);

    // Process B: a new CLI, hydrating state from disk. Only the daemon's
    // buffer can answer this — `XYZ` has never had a newline, and a screen
    // snapshot of a line-discipline-held payload would be empty.
    let first = read_pane_until(&xscore, &pid, |s| s == "XYZ");
    assert_eq!(
        first, "XYZ",
        "a cross-process read must return the bytes written, not the screen: {first:?}"
    );

    // A burst that cannot fit on three rows: the screen forgets the start, the
    // stream does not.
    let burst = home_for(name).join("burst.txt");
    std::fs::write(&burst, b"\nline1\nline2\nline3\nline4\nline5\n").unwrap();
    write_pane(&xscore, &pid, &burst);

    let all = read_pane_until(&xscore, &pid, |s| s.contains("line5"));
    assert!(
        all.starts_with("XYZ\nline1"),
        "the buffer must hold the stream from its first byte: {all:?}"
    );
    assert!(
        all.lines().count() > 3,
        "only {} lines: this looks like a screen snapshot, not the stream",
        all.lines().count()
    );
    let screen = tmux_stdout(name, &["capture-pane", "-p", "-t", "work"]);
    assert!(
        !screen.contains("XYZ"),
        "the screen still shows what the read returned, so this proves nothing: {screen:?}"
    );

    let _ = xscore().args(["session", "kill", "work"]).output();
    cleanup_tmux(name);
    cleanup_daemon(name);
}

/// Test 2 — the buffers are daemon memory, exactly like a direct pane's.
///
/// `daemon stop` loses what was buffered and leaves the tmux session alone:
/// re-creating scrollback is cheap, a durability contract is not (arch.md §13).
#[test]
fn xscore_daemon_restart_drops_tmux_buffers() {
    let name = "daemon_tmux_buffers";
    let xscore = || xscore_for(name);
    cleanup_tmux(name);
    cleanup_daemon(name);

    let created = xscore()
        .args([
            "session", "new", "work", "--tmux",
            "--cmd", "stty -echo raw; exec cat",
            "--cols", "40", "--rows", "3",
        ])
        .output()
        .unwrap();
    assert!(ok(&created), "session new --tmux failed: {}", String::from_utf8_lossy(&created.stderr));
    let pid = parse_id_from_stdout(&created.stdout, "pane");

    let payload = home_for(name).join("kept.txt");
    std::fs::write(&payload, b"buffered-in-daemon").unwrap();
    write_pane(&xscore, &pid, &payload);
    let seen = read_pane_until(&xscore, &pid, |s| s == "buffered-in-daemon");
    assert_eq!(seen, "buffered-in-daemon", "the daemon never buffered the pane: {seen:?}");

    let stop = xscore().args(["daemon", "stop"]).output().unwrap();
    assert!(ok(&stop), "daemon stop failed: {}", String::from_utf8_lossy(&stop.stderr));

    // A new daemon has no history for this pane (tmux replays nothing), so the
    // read succeeds and says "nothing arrived" instead of inventing a screen.
    let read = xscore()
        .args(["data", "read", "--pane", &pid, "--bytes", "4096"])
        .output()
        .unwrap();
    assert!(ok(&read), "data read failed: {}", String::from_utf8_lossy(&read.stderr));
    assert!(
        read.stdout.is_empty(),
        "the buffer must be memory, like a native pane's: {:?}",
        String::from_utf8_lossy(&read.stdout)
    );

    // ...and the tmux session itself was never touched.
    let alive = tmux_for(name).args(["has-session", "-t", "work"]).output().unwrap();
    assert!(alive.status.success(), "`daemon stop` must not end the tmux session");

    let _ = xscore().args(["session", "kill", "work"]).output();
    cleanup_tmux(name);
    cleanup_daemon(name);
}

/// Test 3 — `session kill` closes the daemon's slot as well as the server
/// session: a client left attached to a dead session would keep the daemon
/// (and its buffers) alive for nothing.
#[test]
fn xscore_session_kill_removes_the_daemon_slot() {
    let name = "daemon_tmux_slot";
    let xscore = || xscore_for(name);
    cleanup_tmux(name);
    cleanup_daemon(name);

    let created = xscore()
        .args(["session", "new", "work", "--tmux"])
        .output()
        .unwrap();
    assert!(ok(&created), "session new --tmux failed: {}", String::from_utf8_lossy(&created.stderr));

    // Creating a managed session is what brings the daemon up, and it says so.
    let status = xscore().args(["daemon", "status"]).output().unwrap();
    assert!(ok(&status), "daemon status failed: {}", String::from_utf8_lossy(&status.stderr));
    let v = json(&status.stdout);
    let slots = v["status"]["tmux"].as_array().expect("status.tmux array");
    let slot = slots
        .iter()
        .find(|t| t["name"] == serde_json::json!("work"))
        .unwrap_or_else(|| panic!("the daemon must hold the tmux session: {v}"));
    assert_eq!(slot["alive"], serde_json::json!(true), "and its client must be up: {slot}");

    let kill = xscore().args(["session", "kill", "work"]).output().unwrap();
    assert!(ok(&kill), "session kill failed: {}", String::from_utf8_lossy(&kill.stderr));

    // The slot is gone. The daemon may also have idle-exited by now (it has
    // nothing left to serve), which is the same answer.
    let status = xscore().args(["daemon", "status"]).output().unwrap();
    if status.status.success() {
        let v = json(&status.stdout);
        let slots = v["status"]["tmux"].as_array().cloned().unwrap_or_default();
        assert!(
            slots.iter().all(|t| t["name"] != serde_json::json!("work")),
            "the daemon still holds the killed session: {v}"
        );
    } else {
        assert!(
            String::from_utf8_lossy(&status.stderr).contains("no daemon"),
            "status failed for the wrong reason: {}",
            String::from_utf8_lossy(&status.stderr)
        );
    }

    let alive = tmux_for(name).args(["has-session", "-t", "work"]).output().unwrap();
    assert!(!alive.status.success(), "the tmux session survived `session kill`");

    cleanup_tmux(name);
    cleanup_daemon(name);
}
