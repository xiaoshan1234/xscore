// crates/xscore/tests/cli_smoke.rs (v0.4.0)
//
// CLI smoke tests — run the actual `xscore` binary and verify its
// stdout / stderr / exit code. `env!("CARGO_BIN_EXE_xscore")` is
// provided by Cargo for integration tests.
//
// Isolation: every test gets its own HOME (state.json) AND its own
// `XSCORE_SOCKET` (so tmux sessions never touch the user's `xscore`
// server or another test's sessions). The HOME is wiped once per test
// name per process, then left alone — multi-invocation tests rely on
// state.json persisting between calls.
//
// v0.4.0 semantics under test (ADR 0008):
//   - `link add` always yields link + (usually) window + pane
//   - tmux links ADOPT the session's window/pane (`@N` / `%N` handles)
//   - `link add --pane` binds a hollow pane, refuses tmux links, and
//     refuses panes already attached elsewhere
//   - tmux panes cannot be detached; they die with their session

use std::collections::HashSet;
use std::path::PathBuf;
use std::process::Command;
use std::sync::{Mutex, OnceLock};

fn home_for(test_name: &str) -> PathBuf {
    std::env::temp_dir().join(format!("xscore-test-{test_name}"))
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
        let _ = std::fs::remove_dir_all(home.join(".config").join("xscore"));
    }
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_xscore"));
    cmd.env("HOME", &home);
    cmd.env("XSCORE_SOCKET", format!("xscore-test-{test_name}"));
    cmd
}

/// Kill this test's tmux server (best effort).
fn cleanup_tmux(test_name: &str) {
    let _ = Command::new("tmux")
        .args(["-L", &format!("xscore-test-{test_name}"), "kill-server"])
        .output();
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

#[test]
fn xscore_link_open_unsupported_for_shell() {
    let xscore = || xscore_for("link_open_unshell");
    let out = xscore()
        .args(["link", "open", "shell:///bin/cat"])
        .output()
        .unwrap();
    assert!(!out.status.success(), "shell + `link open` must fail");
    let s = String::from_utf8_lossy(&out.stderr);
    assert!(
        s.contains("tmux") || s.contains("OpenOnlyForTmux"),
        "expected OpenOnlyForTmux error, got: {s}"
    );
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
    assert!(pane["sink_handle"].is_null(), "direct pane must have no tmux handle: {pane}");

    // Window is virtual: no tmux handle.
    let window = json(&xscore()
        .args(["window", "show", "--id", &window_id])
        .output()
        .unwrap()
        .stdout);
    assert!(window["tmux_window_handle"].is_null(), "virtual window: {window}");

    // The link really is the raw sink.
    let link = json(&xscore()
        .args(["link", "show", "--id", &link_id])
        .output()
        .unwrap()
        .stdout);
    assert_eq!(link["sink"]["sink"], serde_json::json!("raw"));

    let _ = xscore().args(["link", "del", "--id", &link_id]).output();
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
}

// ----- v0.4.0 #4: tmux link adopts the session's window and pane -----

#[test]
fn xscore_link_add_tmux_adopts_window_and_pane() {
    let xscore = || xscore_for("link_add_tmux_adopt");
    let out = xscore()
        .args(["link", "add", "shell:///bin/cat?tmux=on"])
        .output()
        .unwrap();
    assert!(ok(&out), "tmux add failed: {}", String::from_utf8_lossy(&out.stderr));

    let link_id = parse_id_from_stdout(&out.stdout, "link");
    let window_id = parse_id_from_stdout(&out.stdout, "window");
    let pane_id = parse_id_from_stdout(&out.stdout, "pane");

    let pane = json(&xscore()
        .args(["pane", "show", "--id", &pane_id])
        .output()
        .unwrap()
        .stdout);
    let handle = pane["sink_handle"].as_str().expect("sink_handle present");
    assert!(handle.starts_with('%'), "pane handle should be %N, got {handle}");
    assert_eq!(pane["link_id"], serde_json::json!(link_id));
    assert_eq!(pane["window_id"], serde_json::json!(window_id));

    let window = json(&xscore()
        .args(["window", "show", "--id", &window_id])
        .output()
        .unwrap()
        .stdout);
    let wh = window["tmux_window_handle"].as_str().expect("tmux_window_handle present");
    assert!(wh.starts_with('@'), "window handle should be @N, got {wh}");
    assert!(window["cols"].as_u64().unwrap() > 0, "geometry adopted from tmux: {window}");

    // `link sync` on a freshly-adopted link is clean.
    let sync = xscore()
        .args(["link", "sync", "--id", &link_id])
        .output()
        .unwrap();
    assert!(ok(&sync));
    assert!(
        String::from_utf8_lossy(&sync.stdout).contains("in sync"),
        "expected clean sync: {}",
        String::from_utf8_lossy(&sync.stdout)
    );

    let _ = xscore().args(["link", "del", "--id", &link_id]).output();
    cleanup_tmux("link_add_tmux_adopt");
}

// ----- v0.4.0 #5: `--pane` is rejected for tmux links -----

#[test]
fn xscore_link_add_tmux_with_pane_errors() {
    let xscore = || xscore_for("link_add_tmux_with_pane");
    let pane_id = parse_id_from_stdout(
        &xscore().args(["pane", "add"]).output().unwrap().stdout,
        "pane",
    );
    let out = xscore()
        .args(["link", "add", "shell:///bin/cat?tmux=on", "--pane", &pane_id])
        .output()
        .unwrap();
    assert!(!out.status.success(), "tmux link + --pane must fail");
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        err.contains("create panes via tmux") || err.contains("TmuxLinkCannotBindPane"),
        "expected TmuxLinkCannotBindPane, got: {err}"
    );
    // The rejected call must not leave a pane bound or a session behind.
    let pane = json(&xscore()
        .args(["pane", "show", "--id", &pane_id])
        .output()
        .unwrap()
        .stdout);
    assert!(pane["link_id"].is_null(), "pane must stay orphan: {pane}");

    let _ = xscore().args(["pane", "del", "--id", &pane_id]).output();
    cleanup_tmux("link_add_tmux_with_pane");
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
    assert!(w["tmux_window_handle"].is_null(), "virtual window: {w}");
    assert_eq!(w["cols"], serde_json::json!(100));
    assert_eq!(w["layout"]["kind"], serde_json::json!("single"));
    let _ = xscore().args(["window", "del", "--id", &wid]).output();
}

// ----- v0.4.0 #7: tmux-bound window gets a real @N handle -----

#[test]
fn xscore_window_add_tmux_bound() {
    let xscore = || xscore_for("window_add_tmux_bound");
    let link_id = parse_id_from_stdout(
        &xscore()
            .args(["link", "add", "shell:///bin/cat?tmux=on"])
            .output()
            .unwrap()
            .stdout,
        "link",
    );
    let out = xscore()
        .args(["window", "add", "--name", "logs", "--link", &link_id])
        .output()
        .unwrap();
    assert!(ok(&out), "window add --link failed: {}", String::from_utf8_lossy(&out.stderr));
    let wid = parse_id_from_stdout(&out.stdout, "window");

    let w = json(&xscore()
        .args(["window", "show", "--id", &wid])
        .output()
        .unwrap()
        .stdout);
    let handle = w["tmux_window_handle"].as_str().expect("tmux handle");
    assert!(handle.starts_with('@'), "expected @N, got {handle}");
    assert_eq!(w["name"], serde_json::json!("logs"));

    let _ = xscore().args(["link", "del", "--id", &link_id]).output();
    cleanup_tmux("window_add_tmux_bound");
}

// ----- v0.4.0 #8: --link on a direct link is an error -----

#[test]
fn xscore_window_add_direct_link_errors() {
    let xscore = || xscore_for("window_add_direct_errors");
    let link_id = parse_id_from_stdout(
        &xscore()
            .args(["link", "add", "shell:///bin/cat"])
            .output()
            .unwrap()
            .stdout,
        "link",
    );
    let out = xscore()
        .args(["window", "add", "--link", &link_id])
        .output()
        .unwrap();
    assert!(!out.status.success(), "window add --link <direct> must fail");
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        err.contains("does not use the tmux sink") || err.contains("NotTmuxLink"),
        "expected NotTmuxLink, got: {err}"
    );
    let _ = xscore().args(["link", "del", "--id", &link_id]).output();
}

// ----- v0.4.0 #9: `pane add --window` splits a real tmux window -----

#[test]
fn xscore_pane_add_to_tmux_window() {
    let xscore = || xscore_for("pane_add_tmux_window");
    let add = xscore()
        .args(["link", "add", "shell:///bin/cat?tmux=on"])
        .output()
        .unwrap();
    assert!(ok(&add));
    let link_id = parse_id_from_stdout(&add.stdout, "link");
    let window_id = parse_id_from_stdout(&add.stdout, "window");
    let adopted = parse_id_from_stdout(&add.stdout, "pane");

    let out = xscore()
        .args(["pane", "add", "--window", &window_id])
        .output()
        .unwrap();
    assert!(ok(&out), "pane add --window failed: {}", String::from_utf8_lossy(&out.stderr));
    let pid = parse_id_from_stdout(&out.stdout, "pane");
    assert_ne!(pid, adopted);

    let pane = json(&xscore()
        .args(["pane", "show", "--id", &pid])
        .output()
        .unwrap()
        .stdout);
    let handle = pane["sink_handle"].as_str().expect("sink_handle");
    assert!(handle.starts_with('%'), "expected %N, got {handle}");
    assert_eq!(pane["link_id"], serde_json::json!(link_id));
    assert_eq!(pane["window_id"], serde_json::json!(window_id));

    let _ = xscore().args(["link", "del", "--id", &link_id]).output();
    cleanup_tmux("pane_add_tmux_window");
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
    assert!(pane["sink_handle"].is_null(), "{pane}");
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
}

// ----- v0.4.0 #12: link del removes tmux panes with the session -----

#[test]
fn xscore_link_del_tmux_removes_panes() {
    let xscore = || xscore_for("link_del_tmux_removes");
    let add = xscore()
        .args(["link", "add", "shell:///bin/cat?tmux=on"])
        .output()
        .unwrap();
    assert!(ok(&add));
    let link_id = parse_id_from_stdout(&add.stdout, "link");
    let pane_id = parse_id_from_stdout(&add.stdout, "pane");

    assert!(ok(&xscore().args(["link", "del", "--id", &link_id]).output().unwrap()));

    let show = xscore().args(["pane", "show", "--id", &pane_id]).output().unwrap();
    assert!(!show.status.success(), "tmux pane must be gone after link del");
    assert!(
        String::from_utf8_lossy(&show.stderr).contains("pane not found"),
        "expected PaneNotFound, got: {}",
        String::from_utf8_lossy(&show.stderr)
    );
    cleanup_tmux("link_del_tmux_removes");
}

// ----- v0.4.0 #13: tmux panes cannot be detached -----

#[test]
fn xscore_tmux_pane_cannot_detach() {
    let xscore = || xscore_for("tmux_pane_cannot_detach");
    let add = xscore()
        .args(["link", "add", "shell:///bin/cat?tmux=on"])
        .output()
        .unwrap();
    assert!(ok(&add));
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

    let _ = xscore().args(["link", "del", "--id", &link_id]).output();
    cleanup_tmux("tmux_pane_cannot_detach");
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
    assert!(err.contains("expected 2"), "got: {err}");
    assert!(err.contains("delete"), "error should tell the user what to do: {err}");

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
}

// ----- v0.4.1 (P0-3): binding survives a restart, connection does not -----

#[test]
fn xscore_direct_link_reports_not_connected_after_restart() {
    let xscore = || xscore_for("direct_not_connected");

    // Process 1: create a direct link. It auto-creates a window + pane,
    // and the pane is genuinely attached *in this process*.
    let add = xscore().args(["link", "add", "shell:///bin/cat"]).output().unwrap();
    assert!(ok(&add));
    let link_id = parse_id_from_stdout(&add.stdout, "link");
    let pane_id = parse_id_from_stdout(&add.stdout, "pane");

    // Process 2: the transport died with process 1. `link show` must
    // say so instead of presenting a dead link as a live one.
    let show = xscore().args(["link", "show", "--id", &link_id]).output().unwrap();
    assert!(ok(&show));
    let link = json(&show.stdout);
    assert_eq!(link["connected"], serde_json::json!(false),
        "hydrated direct link must report connected=false: {link}");

    // The pane's *binding* is still real — that part must not be erased.
    let pane = json(&xscore()
        .args(["pane", "show", "--id", &pane_id])
        .output().unwrap().stdout);
    assert_eq!(pane["link_id"], serde_json::json!(link_id),
        "binding must survive the restart: {pane}");

    // And data ops must explain the real problem (connection lost), not
    // claim the pane was never attached.
    let write = xscore()
        .args(["data", "write", "--pane", &pane_id])
        .output().unwrap();
    assert!(!write.status.success(), "write on a disconnected link must fail");
    let err = String::from_utf8_lossy(&write.stderr);
    assert!(
        err.contains("not connected") || err.contains("LinkNotConnected"),
        "expected LinkNotConnected, got: {err}"
    );
    assert!(
        !err.contains("not attached"),
        "must not report NotAttached for a bound pane: {err}"
    );

    let _ = xscore().args(["link", "del", "--id", &link_id]).output();
    let _ = xscore().args(["pane", "del", "--id", &pane_id]).output();
}
