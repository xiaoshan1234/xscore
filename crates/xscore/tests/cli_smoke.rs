// crates/xscore/tests/cli_smoke.rs
//
// CLI smoke tests — run the actual `xscore` binary and verify its
// stdout / stderr / exit code. These tests use `env!("CARGO_BIN_EXE_xscore")`
// which Cargo provides for integration tests.
//
// State persistence caveat: each `xscore` invocation is a fresh process
// with a fresh `Arc<SessionManager>`. Tests that span multiple
// invocations (e.g. `link add` followed by `link list`) are marked
// `#[ignore]` because they require the in-memory state to survive —
// that lands in task-15 (state persistence). To run them manually:
//
//   cargo test --test cli_smoke -- --ignored
//
// (and the implementation behind them must also be landed.)
//
// Currently-passing tests (5/10) cover the parts of the CLI that work
// today: help text, input validation, error invariants, and the
// disk-based `link config` round-trip.

use std::process::Command;

fn xscore() -> Command {
    Command::new(env!("CARGO_BIN_EXE_xscore"))
}

#[test]
fn xscore_help_exits_zero() {
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
    let out = xscore().args(["link", "add"]).output().unwrap();
    assert!(!out.status.success(), "`link add` with no inputs must fail");
}

#[test]
fn xscore_link_open_unsupported_for_shell() {
    // ADR 0005: `link open` is tmux-only; shell/serial/ssh return OpenOnlyForTmux.
    // (After the manager.rs `open_link` fix that checks `address.kind() != "tmux"`,
    // this returns a clean error instead of a `todo!()` panic.)
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

// ----- ADR 0005 invariant: two `link add <same url>` = two distinct links -----
// No state needed: each invocation's process creates a fresh link, and
// we only compare the IDs (which are unique per call).

#[test]
fn xscore_link_add_default_creates_two_links_for_same_url() {
    let out1 = xscore()
        .args(["link", "add", "shell:///bin/echo"])
        .output()
        .unwrap();
    let id1 = String::from_utf8_lossy(&out1.stdout)
        .lines()
        .find(|l| l.starts_with("link:"))
        .unwrap_or_else(|| {
            panic!(
                "first `link add` produced no `link:` line. stdout:\n{}\nstderr:\n{}",
                String::from_utf8_lossy(&out1.stdout),
                String::from_utf8_lossy(&out1.stderr),
            )
        })
        .trim()
        .strip_prefix("link:")
        .unwrap()
        .trim()
        .to_string();

    let out2 = xscore()
        .args(["link", "add", "shell:///bin/echo"])
        .output()
        .unwrap();
    let id2 = String::from_utf8_lossy(&out2.stdout)
        .lines()
        .find(|l| l.starts_with("link:"))
        .unwrap_or_else(|| {
            panic!(
                "second `link add` produced no `link:` line. stdout:\n{}\nstderr:\n{}",
                String::from_utf8_lossy(&out2.stdout),
                String::from_utf8_lossy(&out2.stderr),
            )
        })
        .trim()
        .strip_prefix("link:")
        .unwrap()
        .trim()
        .to_string();

    assert_ne!(id1, id2, "two add() calls should produce distinct link_ids");

    // Cleanup is best-effort; without state persistence these will fail
    // (no manager remembers the links), but we don't care.
    let _ = xscore().args(["link", "close", "--id", &id1]).output();
    let _ = xscore().args(["link", "close", "--id", &id2]).output();
}

// ----- `link config` template (disk-based, no in-memory state needed) -----

#[test]
fn xscore_link_config_add_and_list_roundtrip() {
    // Clean up any prior template from a previous test run.
    let _ = std::fs::remove_dir_all(
        dirs::config_dir().unwrap().join("xscore").join("links"),
    );
    let add = xscore()
        .args(["link", "config", "add", "web", "ssh://root@host:22"])
        .output()
        .unwrap();
    assert!(
        add.status.success(),
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

// ============================================================
// Tests below this line require state persistence (task-15 / F)
// — each `xscore` invocation is a fresh process with a fresh
// `Arc<SessionManager>`, so the manager doesn't remember the
// links/windows/panes from a previous invocation. They are
// marked `#[ignore]` and will be re-enabled once F lands.
// ============================================================

#[test]
#[ignore = "requires state persistence (task-15 / F)"]
fn xscore_link_add_creates_link_and_pane() {
    let out = xscore()
        .args(["link", "add", "shell:///bin/cat"])
        .output()
        .unwrap();
    assert!(out.status.success());
    let s = String::from_utf8_lossy(&out.stdout);
    assert!(s.contains("link:"));
    assert!(s.contains("pane:"));

    let link_id = s
        .lines()
        .find(|l| l.starts_with("link:"))
        .unwrap()
        .trim()
        .strip_prefix("link:")
        .unwrap()
        .trim()
        .to_string();
    let pane_id = s
        .lines()
        .find(|l| l.starts_with("pane:"))
        .unwrap()
        .trim()
        .strip_prefix("pane:")
        .unwrap()
        .trim()
        .to_string();

    let list_out = xscore().args(["link", "list"]).output().unwrap();
    assert!(String::from_utf8_lossy(&list_out.stdout).contains(&link_id));

    let del_pane = xscore()
        .args(["pane", "del", "--id", &pane_id])
        .output()
        .unwrap();
    assert!(del_pane.status.success());

    let del_link = xscore()
        .args(["link", "del", "--id", &link_id])
        .output()
        .unwrap();
    assert!(del_link.status.success());
}

#[test]
#[ignore = "requires state persistence (task-15 / F)"]
fn xscore_link_close_unsupported_for_shell() {
    let out = xscore()
        .args(["link", "add", "shell:///bin/cat"])
        .output()
        .unwrap();
    let link_id = String::from_utf8_lossy(&out.stdout)
        .lines()
        .find(|l| l.starts_with("link:"))
        .unwrap()
        .trim()
        .strip_prefix("link:")
        .unwrap()
        .trim()
        .to_string();

    let close = xscore()
        .args(["link", "close", "--id", &link_id])
        .output()
        .unwrap();
    assert!(!close.status.success(), "shell + `link close` must fail");
    let s = String::from_utf8_lossy(&close.stderr);
    assert!(
        s.contains("tmux") || s.contains("CloseOnlyForTmux"),
        "expected CloseOnlyForTmux error, got: {s}"
    );

    let _ = xscore().args(["link", "del", "--id", &link_id]).output();
}

#[test]
#[ignore = "requires state persistence (task-15 / F)"]
fn xscore_link_del_kills_link() {
    let add = xscore()
        .args(["link", "add", "shell:///bin/cat"])
        .output()
        .unwrap();
    let link_id = String::from_utf8_lossy(&add.stdout)
        .lines()
        .find(|l| l.starts_with("link:"))
        .unwrap()
        .trim()
        .strip_prefix("link:")
        .unwrap()
        .trim()
        .to_string();

    let del = xscore()
        .args(["link", "del", "--id", &link_id])
        .output()
        .unwrap();
    assert!(del.status.success());

    let list = xscore().args(["link", "list"]).output().unwrap();
    assert!(
        !String::from_utf8_lossy(&list.stdout).contains(&link_id),
        "link still in registry after del: {link_id}"
    );
}

#[test]
#[ignore = "requires state persistence (task-15 / F)"]
fn xscore_window_add_inside_link() {
    let add = xscore()
        .args(["link", "add", "shell:///bin/bash"])
        .output()
        .unwrap();
    let s = String::from_utf8_lossy(&add.stdout);
    let link_id = s
        .lines()
        .find(|l| l.starts_with("link:"))
        .unwrap()
        .trim()
        .strip_prefix("link:")
        .unwrap()
        .trim()
        .to_string();

    let window = xscore()
        .args(["window", "add", "--link", &link_id, "--name", "dev"])
        .output()
        .unwrap();
    assert!(window.status.success());
    let ws = String::from_utf8_lossy(&window.stdout);
    assert!(ws.contains("window:"));
    let wid = ws
        .lines()
        .find(|l| l.starts_with("window:"))
        .unwrap()
        .trim()
        .strip_prefix("window:")
        .unwrap()
        .trim()
        .to_string();

    let list = xscore()
        .args(["window", "list", "--link", &link_id])
        .output()
        .unwrap();
    assert!(String::from_utf8_lossy(&list.stdout).contains(&wid));

    let _ = xscore().args(["window", "del", "--id", &wid]).output();
    let _ = xscore().args(["link", "del", "--id", &link_id]).output();
}

#[test]
#[ignore = "requires state persistence (task-15 / F)"]
fn xscore_window_del_kills_window() {
    let add = xscore()
        .args(["link", "add", "shell:///bin/cat"])
        .output()
        .unwrap();
    let s = String::from_utf8_lossy(&add.stdout);
    let link_id = s
        .lines()
        .find(|l| l.starts_with("link:"))
        .unwrap()
        .trim()
        .strip_prefix("link:")
        .unwrap()
        .trim()
        .to_string();
    let initial_window = s
        .lines()
        .find(|l| l.starts_with("window:"))
        .unwrap()
        .trim()
        .strip_prefix("window:")
        .unwrap()
        .trim()
        .to_string();
    let del = xscore()
        .args(["window", "del", "--id", &initial_window])
        .output()
        .unwrap();
    assert!(del.status.success());

    let list = xscore()
        .args(["window", "list", "--link", &link_id])
        .output()
        .unwrap();
    assert!(
        !String::from_utf8_lossy(&list.stdout).contains(&initial_window),
        "deleted window still in registry: {initial_window}"
    );

    let _ = xscore().args(["link", "del", "--id", &link_id]).output();
}

#[test]
#[ignore = "requires state persistence (task-15 / F)"]
fn xscore_pane_add_uses_window_not_link() {
    let add = xscore()
        .args(["link", "add", "shell:///bin/echo"])
        .output()
        .unwrap();
    let s = String::from_utf8_lossy(&add.stdout);
    let window_id = s
        .lines()
        .find(|l| l.starts_with("window:"))
        .unwrap()
        .trim()
        .strip_prefix("window:")
        .unwrap()
        .trim()
        .to_string();

    let pane = xscore()
        .args(["pane", "add", "--window", &window_id, "--name", "bash"])
        .output()
        .unwrap();
    assert!(pane.status.success());
    assert!(String::from_utf8_lossy(&pane.stdout).contains("pane:"));
}
