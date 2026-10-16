# Task 13: Integration tests + README

**Files:**
- Create: `crates/xscore/tests/cli_smoke.rs`
- Modify: `README.md`

**Step 1: CLI smoke test**

```rust
// tests/cli_smoke.rs
use std::process::Command;

#[test]
fn xscore_help_exits_zero() {
    let out = Command::new(env!("CARGO_BIN_EXE_xscore")).arg("--help").output().unwrap();
    assert!(out.status.success());
    let s = String::from_utf8(out.stdout).unwrap();
    assert!(s.contains("connect"));
    assert!(s.contains("link"));
    assert!(s.contains("pane"));
}

#[test]
fn xscore_connect_creates_link_and_pane() {
    let out = Command::new(env!("CARGO_BIN_EXE_xscore"))
        .args(["connect", "shell:///bin/cat"]).output().unwrap();
    assert!(out.status.success());
    let s = String::from_utf8_lossy(&out.stdout);
    assert!(s.contains("link:"));
    assert!(s.contains("pane:"));

    let link_line = s.lines().find(|l| l.starts_with("link:")).unwrap();
    let pane_line = s.lines().find(|l| l.starts_with("pane:")).unwrap();
    let link_id = link_line.trim().strip_prefix("link:").unwrap().trim();
    let pane_id = pane_line.trim().strip_prefix("pane:").unwrap().trim();

    let list_out = Command::new(env!("CARGO_BIN_EXE_xscore"))
        .args(["link", "list"]).output().unwrap();
    assert!(String::from_utf8_lossy(&list_out.stdout).contains(link_id));

    let close_pane = Command::new(env!("CARGO_BIN_EXE_xscore"))
        .args(["pane", "close", "--id", pane_id]).output().unwrap();
    assert!(close_pane.status.success());

    let close_link = Command::new(env!("CARGO_BIN_EXE_xscore"))
        .args(["link", "close", "--id", link_id]).output().unwrap();
    assert!(close_link.status.success());
}

#[test]
fn xscore_connect_default_creates_two_links_for_same_url() {
    // First connect
    let out1 = Command::new(env!("CARGO_BIN_EXE_xscore"))
        .args(["connect", "shell:///bin/echo"]).output().unwrap();
    let id1 = String::from_utf8_lossy(&out1.stdout).lines()
        .find(|l| l.starts_with("link:")).unwrap().trim().strip_prefix("link:").unwrap().trim().to_string();

    // Second connect with same URL but no --link=existing → second link
    let out2 = Command::new(env!("CARGO_BIN_EXE_xscore"))
        .args(["connect", "shell:///bin/echo"]).output().unwrap();
    let id2 = String::from_utf8_lossy(&out2.stdout).lines()
        .find(|l| l.starts_with("link:")).unwrap().trim().strip_prefix("link:").unwrap().trim().to_string();

    assert_ne!(id1, id2, "two connect() calls should produce distinct link_ids");

    // Cleanup
    let _ = Command::new(env!("CARGO_BIN_EXE_xscore"))
        .args(["link", "close", "--id", &id1]).output();
    let _ = Command::new(env!("CARGO_BIN_EXE_xscore"))
        .args(["link", "close", "--id", &id2]).output();
}

#[test]
fn xscore_link_existing_unsupported_for_shell() {
    let out = Command::new(env!("CARGO_BIN_EXE_xscore"))
        .args(["connect", "shell:///bin/cat", "--link-existing"]).output().unwrap();
    assert!(!out.status.success(), "shell + --link=existing must fail");
    let s = String::from_utf8_lossy(&out.stderr);
    assert!(s.contains("not supported") || s.contains("LinkExistingUnsupported"),
            "expected LinkExistingUnsupported error, got: {s}");
}
```

**Step 2: README**

```markdown
# xscore

Rust session manager for SSH / shell / serial, modeled as **Link x Pane**.

- **Link** = one physical connection (SSH channel, serial port, local child, or tmux server).
- **Pane** = one working view into a link. N panes per link; each pane has its own id, snapshot, write state.

## Quick start

\`\`\`
xscore connect shell:///bin/bash
xscore connect ssh://user@host:22
xscore connect serial:///dev/ttyUSB0?baud=115200

# Multiple panes on the same link:
xscore connect ssh://u@h:22
xscore connect ssh://u@h:22 --link=existing   # opens a 2nd pane in the same tmux server
\`\`\`

## Commands

\`\`\`
xscore connect <url> [--save] [--link=existing]
xscore link list
xscore link show --id <link-id>
xscore link close --id <link-id>
xscore link save --id <link-id>
xscore link load --path <file>

xscore pane list [--link <link-id>]
xscore pane show --id <pane-id>
xscore pane close --id <pane-id>

xscore data read --pane <pane-id> [--bytes N]
xscore data write --pane <pane-id> [--from-file FILE]
xscore data resize --pane <pane-id> --cols C --rows R

xscore config show
xscore config path
xscore config set <key> <value>
xscore config unset <key>
\`\`\`

## Architecture

\`\`\`
CLI -> SessionManager
       +-- LinkRegistry     (address-keyed dedup)
       |    +-- Link { transport + sink + Vec<PaneId> }
       +-- PaneRegistry     (pane_id -> PaneInfo)
       +-- BackendRegistry  (pane_id -> Arc<dyn PaneBackend>)
                                  ^ wraps (Link, sink_handle)
\`\`\`

Each Link owns one Transport (raw IO) + one Sink (PTY/ANSI/tmux). Each Pane lives inside
one link and addresses the link's sink via a sink_handle (e.g. tmux `%42`). Closing a
pane does not close the link; closing a link drops every pane on it.

`--link=existing` opts into **address-keyed dedup**: search LinkRegistry for an existing
link matching the URL's transport address; if found, open a new pane on it; if not found,
error. Without `--link=existing`, every `xscore connect` creates a brand-new link.
```

**Step 4: Run all tests**

Run: `cargo test -p xscore`
Expected: all unit + integration tests pass.

**Step 5: Commit**

```bash
git add tests/ README.md
git commit -m "test+docs: Link x Pane smoke tests + README"
```