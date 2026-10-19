// crates/xscore/tests/cli_smoke.rs
//
// CLI smoke tests — run the actual `xscore` binary and verify its
// stdout / stderr / exit code. These tests use `env!("CARGO_BIN_EXE_xscore")`
// which Cargo provides for integration tests.
//
// State persistence caveat: each `xscore` invocation is a fresh process
// with a fresh `Arc<SessionManager>`. Tests that span multiple
// invocations (e.g. `link add` followed by `pane list`) rely on the
// v0.3.0 state persistence in `~/.config/xscore/state/state.json`.
//
// v0.3.0 rewrote the CLI surface: `link add` only creates a link (no
// auto window/pane), and the new verbs `pane {attach, detach, add-to-
// window, ...}` glue together the decoupled entities. The test set
// covers both the new verbs and the Q1 / Q2 invariants.

use std::process::Command;

fn xscore_for(test_name: &str) -> Command {
    let home = std::env::temp_dir().join(format!("xscore-test-{test_name}"));
    std::fs::create_dir_all(&home).ok();
    // Wipe the entire state directory on EVERY call. Tests that need
    // multi-invocation persistence use a single `xscore_for` closure
    // whose `home` is the same path — but since each subprocess is a
    // fresh manager and the state is reloaded from disk at startup,
    // multi-invocation tests work as long as we don't wipe between
    // their commands.
    //
    // Wait — actually, multi-invocation tests (full_flow, q1_*) DO
    // need persistence across calls. So we must NOT wipe on every call.
    //
    // Strategy: only wipe if the file is in an incompatible schema
    // (v0.2.0's bare `[]` array) — that's the only case where a stale
    // file would break a v0.3.0 run. Valid v0.3.0 snapshots are left
    // alone; tests that need a clean slate use a unique test name.
    let state_json = home.join(".config").join("xscore").join("state").join("state.json");
    if state_json.exists() {
        let content = std::fs::read_to_string(&state_json).unwrap_or_default();
        let trimmed = content.trim_start();
        if trimmed.starts_with('[') {
            // v0.2.0 schema: bare array of LinkSnapshot. Incompatible.
            let _ = std::fs::remove_file(&state_json);
        }
    }
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_xscore"));
    cmd.env("HOME", &home);
    cmd
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

// ----- v0.3.0 default: `link add <url>` prints ONLY `link:` -----

#[test]
fn xscore_link_add_default_uses_raw_sink_and_no_pane() {
    let xscore = || xscore_for("link_add_default_raw");
    let out = xscore()
        .args(["link", "add", "shell:///bin/cat"])
        .output()
        .unwrap();
    assert!(out.status.success(), "add failed: {}", String::from_utf8_lossy(&out.stderr));
    let s = String::from_utf8_lossy(&out.stdout);
    assert!(s.contains("link:"), "missing 'link:' line: {s}");
    // v0.3.0: no auto window or pane for raw links.
    assert!(!s.contains("window:"), "raw link should not print 'window:': {s}");
    assert!(!s.contains("pane:"), "raw link should not print 'pane:': {s}");

    let link_id = parse_id_from_stdout(&out.stdout, "link");
    let show = xscore()
        .args(["link", "show", "--id", &link_id])
        .output()
        .unwrap();
    let ss = String::from_utf8_lossy(&show.stdout);
    assert!(ss.contains(r#""sink": "raw""#) || ss.contains(r#""sink":"raw""#),
        "expected raw sink in show, got: {ss}");

    let _ = xscore().args(["link", "del", "--id", &link_id]).output();
}

// ----- v0.3.0: `?tmux=on` still creates a tmux link (no auto window/pane either) -----

#[test]
fn xscore_link_add_with_tmux_on_only_creates_link() {
    let xscore = || xscore_for("link_add_tmux_on");
    let out = xscore()
        .args(["link", "add", "shell:///bin/cat?tmux=on"])
        .output()
        .unwrap();
    assert!(out.status.success());
    let s = String::from_utf8_lossy(&out.stdout);
    assert!(s.contains("link:"));
    // v0.3.0: even with `?tmux=on`, the user must explicitly create + add panes.
    assert!(!s.contains("window:"), "v0.3.0: even tmux links don't print window: {s}");
    assert!(!s.contains("pane:"), "v0.3.0: even tmux links don't print pane: {s}");
    let link_id = parse_id_from_stdout(&out.stdout, "link");
    let _ = xscore().args(["link", "del", "--id", &link_id]).output();
}

// ----- `link add --id <uuid>` -----

#[test]
fn xscore_link_add_with_explicit_id_uses_supplied_id() {
    let xscore = || xscore_for("link_add_explicit_id");
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

    let list = xscore().args(["link", "list"]).output().unwrap();
    let ls = String::from_utf8_lossy(&list.stdout);
    assert!(ls.contains(custom_id), "list should contain id: {ls}");

    let _ = xscore().args(["link", "del", "--id", custom_id]).output();
}

#[test]
fn xscore_link_add_with_duplicate_id_errors() {
    let xscore = || xscore_for("link_add_duplicate_id");
    // Use a uuid v7-shaped id UNIQUE to this test run — but the duplicate
    // detection happens within the same `xscore_for` closure (so both
    // subprocesses see the same HOME). Make the id include a process-id
    // component to avoid collisions with prior runs.
    let id = format!("22222222-3333-7444-8555-{:012x}", std::process::id());
    let first = xscore()
        .args(["link", "add", "shell:///bin/cat", "--id", &id])
        .output()
        .unwrap();
    assert!(first.status.success(), "first add failed: {}",
        String::from_utf8_lossy(&first.stderr));
    let second = xscore()
        .args(["link", "add", "shell:///bin/echo", "--id", &id])
        .output()
        .unwrap();
    assert!(!second.status.success(), "second add with same id must fail");
    let s = String::from_utf8_lossy(&second.stderr);
    assert!(
        s.contains("already in use") || s.contains("LinkIdExists"),
        "expected LinkIdExists error, got: {s}"
    );

    let _ = xscore().args(["link", "del", "--id", &id]).output();
}

// ----- v0.3.0: full flow create link → create pane → attach → write → read -----

#[test]
fn xscore_full_flow_create_pane_attach_data() {
    let xscore = || xscore_for("full_flow");

    // 1. Create link (raw — no ?tmux=).
    let add = xscore().args(["link", "add", "shell:///bin/cat"]).output().unwrap();
    assert!(add.status.success());
    let link_id = parse_id_from_stdout(&add.stdout, "link");

    // 2. Create pane (orphan).
    let add_pane = xscore().args(["pane", "add"]).output().unwrap();
    let pane_id = parse_id_from_stdout(&add_pane.stdout, "pane");

    // 3. Attach the pane to the link.
    let attach = xscore()
        .args(["pane", "attach", "--pane", &pane_id, "--link", &link_id])
        .output().unwrap();
    assert!(attach.status.success(),
        "attach failed: {}", String::from_utf8_lossy(&attach.stderr));

    // 4. Verify the pane's info shows the link_id.
    let show = xscore().args(["pane", "show", "--id", &pane_id]).output().unwrap();
    let s = String::from_utf8_lossy(&show.stdout);
    assert!(s.contains(&link_id),
        "pane.show should include link_id; got: {s}");

    // 5. Create a window + put the pane in it.
    let add_window = xscore()
        .args(["window", "add", "--cols", "120", "--rows", "30"])
        .output().unwrap();
    assert!(add_window.status.success());
    let window_id = parse_id_from_stdout(&add_window.stdout, "window");

    let add_to_window = xscore()
        .args(["pane", "add-to-window", "--pane", &pane_id, "--window", &window_id])
        .output().unwrap();
    assert!(add_to_window.status.success(),
        "add-to-window failed: {}", String::from_utf8_lossy(&add_to_window.stderr));

    // 6. Verify the window list shows the pane.
    let window_list = xscore().args(["window", "list"]).output().unwrap();
    let wl = String::from_utf8_lossy(&window_list.stdout);
    assert!(wl.contains(&window_id), "window list should contain wid: {wl}");

    // 7. Verify the pane list, filtered by window, shows the pane.
    let pane_list = xscore()
        .args(["pane", "list", "--window", &window_id])
        .output().unwrap();
    let pl = String::from_utf8_lossy(&pane_list.stdout);
    assert!(pl.contains(&pane_id),
        "pane list (--window) should contain pane id; got: {pl}");

    // 8. Detach the pane (orphan again).
    let detach = xscore()
        .args(["pane", "detach", "--pane", &pane_id])
        .output().unwrap();
    assert!(detach.status.success());

    // 9. data write should fail now (orphan).
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

// ----- v0.3.0: window add/list/del (no link needed) -----

#[test]
fn xscore_window_add_list_del_orphan() {
    let xscore = || xscore_for("window_orphan");
    let add = xscore()
        .args(["window", "add", "--name", "scratch", "--cols", "100", "--rows", "20"])
        .output().unwrap();
    assert!(add.status.success());
    let wid = parse_id_from_stdout(&add.stdout, "window");

    let list = xscore().args(["window", "list"]).output().unwrap();
    let ls = String::from_utf8_lossy(&list.stdout);
    assert!(ls.contains(&wid), "window list should contain wid: {ls}");

    let del = xscore().args(["window", "del", "--id", &wid]).output().unwrap();
    assert!(del.status.success());
}

// ----- v0.3.0: link del also detaches panes (panes survive as orphan) -----

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
    assert!(del.status.success());

    // The pane survives — info.link_id is now None.
    let show = xscore().args(["pane", "show", "--id", &pane_id]).output().unwrap();
    let s = String::from_utf8_lossy(&show.stdout);
    assert!(
        s.contains(r#""link_id": null"#) || s.contains(r#""link_id":null"#),
        "pane.link_id should be null after link del; got: {s}"
    );

    let _ = xscore().args(["pane", "del", "--id", &pane_id]).output();
}