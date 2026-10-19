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

/// Build a `Command` for the `xscore` binary, scoped to a per-test
/// `HOME` directory so the state file
/// (`~/.config/xscore/state/state.json`) doesn't collide between
/// parallel tests. Each test should call this once, capture the
/// result in a closure, and use that closure for every invocation
/// in the test — sharing a single HOME across the test's invocations
/// is what makes the state-persistence tests work.
///
/// Example:
/// ```ignore
/// let xscore = || xscore_for("link_add_creates");
/// let out1 = xscore().args(["link", "add", "..."]).output()?;
/// let out2 = xscore().args(["link", "list"]).output()?;
/// ```
fn xscore_for(test_name: &str) -> Command {
    let home = std::env::temp_dir().join(format!("xscore-test-{test_name}"));
    std::fs::create_dir_all(&home).ok();
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_xscore"));
    cmd.env("HOME", &home);
    cmd
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
    let xscore = || xscore_for("link_add_two_same_url");
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
    let xscore = || xscore_for("link_config_roundtrip");
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
fn xscore_link_add_creates_link_and_pane() {
    let xscore = || xscore_for("link_add_creates");
    let out = xscore()
        .args(["link", "add", "shell:///bin/cat?tmux=auto"])
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
fn xscore_link_close_unsupported_for_shell() {
    let xscore = || xscore_for("link_close_unshell");
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
fn xscore_link_del_kills_link() {
    let xscore = || xscore_for("link_del_kills");
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
fn xscore_window_add_inside_link() {
    let xscore = || xscore_for("window_add_inside");
    let add = xscore()
        .args(["link", "add", "shell:///bin/bash?tmux=auto"])
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
fn xscore_window_del_kills_window() {
    let xscore = || xscore_for("window_del_kills");
    let add = xscore()
        .args(["link", "add", "shell:///bin/cat?tmux=auto"])
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
fn xscore_pane_add_uses_window_not_link() {
    let xscore = || xscore_for("pane_add_uses_window");
    let add = xscore()
        .args(["link", "add", "shell:///bin/echo?tmux=auto"])
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

// ----- Default sink is raw (no tmux server) when `?tmux=` is absent -----
// Per the 2026-10-19 design pivot: not every link goes through tmux. The
// user requested "支持 tmux 会话的管理，不是所有 link 都要走 tmux". A fresh
// `xscore link add <url>` should default to direct transport I/O and
// print only `link:` (no `window:` / `pane:`).

#[test]
fn xscore_link_add_default_uses_raw_sink() {
    let xscore = || xscore_for("link_add_default_raw");
    let out = xscore()
        .args(["link", "add", "shell:///bin/cat"])
        .output()
        .unwrap();
    assert!(out.status.success(), "add failed: {}", String::from_utf8_lossy(&out.stderr));
    let s = String::from_utf8_lossy(&out.stdout);
    assert!(s.contains("link:"), "missing 'link:' line: {s}");
    assert!(!s.contains("window:"), "raw link should not print 'window:': {s}");
    assert!(!s.contains("pane:"), "raw link should not print 'pane:': {s}");

    // The link's `link show` should report the sink as raw, not tmux.
    let link_id = s
        .lines()
        .find(|l| l.starts_with("link:"))
        .unwrap()
        .trim()
        .strip_prefix("link:")
        .unwrap()
        .trim()
        .to_string();
    let show = xscore()
        .args(["link", "show", "--id", &link_id])
        .output()
        .unwrap();
    let ss = String::from_utf8_lossy(&show.stdout);
    assert!(ss.contains(r#""sink": "raw""#) || ss.contains(r#""sink":"raw""#),
        "expected raw sink in show, got: {ss}");

    // Cleanup
    let _ = xscore().args(["link", "del", "--id", &link_id]).output();
}

#[test]
fn xscore_window_add_errors_for_raw_link() {
    let xscore = || xscore_for("window_add_raw_link");
    let add = xscore()
        .args(["link", "add", "shell:///bin/cat"])  // default = raw
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

    let window = xscore()
        .args(["window", "add", "--link", &link_id, "--name", "dev"])
        .output()
        .unwrap();
    assert!(!window.status.success(), "window add on raw link must fail");
    let s = String::from_utf8_lossy(&window.stderr);
    assert!(
        s.contains("tmux") || s.contains("NotTmuxLink"),
        "expected NotTmuxLink error, got: {s}"
    );

    let _ = xscore().args(["link", "del", "--id", &link_id]).output();
}

#[test]
fn xscore_link_add_with_tmux_on_still_creates_window_and_pane() {
    // Sanity: opt-in `?tmux=on` keeps the old auto-create behavior.
    let xscore = || xscore_for("link_add_tmux_on");
    let out = xscore()
        .args(["link", "add", "shell:///bin/cat?tmux=on"])
        .output()
        .unwrap();
    assert!(out.status.success());
    let s = String::from_utf8_lossy(&out.stdout);
    assert!(s.contains("link:"));
    assert!(s.contains("window:"));
    assert!(s.contains("pane:"));
    let link_id = s.lines().find(|l| l.starts_with("link:")).unwrap()
        .trim().strip_prefix("link:").unwrap().trim().to_string();
    let _ = xscore().args(["link", "del", "--id", &link_id]).output();
}

// ----- `link add --id <uuid>` — specify a known link id -----
// Per the 2026-10-19 user request: "创建 link 的时候能不能指定 link 的 id".
// Useful for scripted workflows where the id must be known in
// advance (e.g., for reconnect-style flows or deterministic tests).
// Default behavior (no `--id`) is unchanged — auto-generated uuid v7.

#[test]
fn xscore_link_add_with_explicit_id_uses_supplied_id() {
    let xscore = || xscore_for("link_add_explicit_id");
    // Use a uuid v7-shaped string (version digit = 7 in the 3rd group,
    // variant digit = 8/9/a/b in the 4th group) so LinkId::parse accepts it.
    let custom_id = "11111111-2222-7333-8444-555555555555";
    let out = xscore()
        .args(["link", "add", "shell:///bin/cat", "--id", custom_id])
        .output()
        .unwrap();
    assert!(out.status.success(),
        "add with --id failed: {}", String::from_utf8_lossy(&out.stderr));
    let s = String::from_utf8_lossy(&out.stdout);
    assert!(s.contains(custom_id),
        "stdout should contain the supplied id '{custom_id}', got: {s}");

    // Verify the link list also shows the supplied id.
    let list = xscore().args(["link", "list"]).output().unwrap();
    let ls = String::from_utf8_lossy(&list.stdout);
    assert!(ls.contains(custom_id), "list should contain id: {ls}");

    let _ = xscore().args(["link", "del", "--id", custom_id]).output();
}

#[test]
fn xscore_link_add_with_duplicate_id_errors() {
    let xscore = || xscore_for("link_add_duplicate_id");
    let id = "22222222-3333-7444-8555-666666666666";
    // First add succeeds.
    let first = xscore()
        .args(["link", "add", "shell:///bin/cat", "--id", id])
        .output()
        .unwrap();
    assert!(first.status.success(), "first add failed: {}",
        String::from_utf8_lossy(&first.stderr));
    // Second add with the same id fails with LinkIdExists.
    let second = xscore()
        .args(["link", "add", "shell:///bin/echo", "--id", id])
        .output()
        .unwrap();
    assert!(!second.status.success(), "second add with same id must fail");
    let s = String::from_utf8_lossy(&second.stderr);
    assert!(
        s.contains("already in use") || s.contains("LinkIdExists"),
        "expected LinkIdExists error, got: {s}"
    );

    let _ = xscore().args(["link", "del", "--id", id]).output();
}
