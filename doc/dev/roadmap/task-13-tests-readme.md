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
    assert!(s.contains("link"));
    assert!(s.contains("pane"));
    assert!(s.contains("data"));
    assert!(s.contains("config"));
}

#[test]
fn xscore_link_add_creates_link_and_pane() {
    let out = Command::new(env!("CARGO_BIN_EXE_xscore"))
        .args(["link", "add", "shell:///bin/cat"]).output().unwrap();
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
fn xscore_link_add_default_creates_two_links_for_same_url() {
    // First add
    let out1 = Command::new(env!("CARGO_BIN_EXE_xscore"))
        .args(["link", "add", "shell:///bin/echo"]).output().unwrap();
    let id1 = String::from_utf8_lossy(&out1.stdout).lines()
        .find(|l| l.starts_with("link:")).unwrap().trim().strip_prefix("link:").unwrap().trim().to_string();

    // Second add with same URL but no --link=existing → second link
    let out2 = Command::new(env!("CARGO_BIN_EXE_xscore"))
        .args(["link", "add", "shell:///bin/echo"]).output().unwrap();
    let id2 = String::from_utf8_lossy(&out2.stdout).lines()
        .find(|l| l.starts_with("link:")).unwrap().trim().strip_prefix("link:").unwrap().trim().to_string();

    assert_ne!(id1, id2, "two add() calls should produce distinct link_ids");

    // Cleanup
    let _ = Command::new(env!("CARGO_BIN_EXE_xscore"))
        .args(["link", "close", "--id", &id1]).output();
    let _ = Command::new(env!("CARGO_BIN_EXE_xscore"))
        .args(["link", "close", "--id", &id2]).output();
}

#[test]
fn xscore_link_add_with_alias_registers_alias() {
    let out = Command::new(env!("CARGO_BIN_EXE_xscore"))
        .args(["link", "add", "shell:///bin/cat", "-a", "cat1"]).output().unwrap();
    assert!(out.status.success());
    let s = String::from_utf8_lossy(&out.stdout);
    assert!(s.contains("alias: cat1"));

    let list = Command::new(env!("CARGO_BIN_EXE_xscore"))
        .args(["link", "list"]).output().unwrap();
    assert!(String::from_utf8_lossy(&list.stdout).contains("cat1"));
}

#[test]
fn xscore_link_existing_unsupported_for_shell() {
    let out = Command::new(env!("CARGO_BIN_EXE_xscore"))
        .args(["link", "add", "shell:///bin/cat", "--link-existing"]).output().unwrap();
    assert!(!out.status.success(), "shell + --link=existing must fail");
    let s = String::from_utf8_lossy(&out.stderr);
    assert!(s.contains("not supported") || s.contains("LinkExistingUnsupported"),
            "expected LinkExistingUnsupported error, got: {s}");
}

#[test]
fn xscore_link_config_add_and_list_roundtrip() {
    let _ = std::fs::remove_dir_all(
        dirs::config_dir().unwrap().join("xscore").join("links"),
    );
    let add = Command::new(env!("CARGO_BIN_EXE_xscore"))
        .args(["link", "config", "add", "web", "ssh://root@host:22"]).output().unwrap();
    assert!(add.status.success(), "link config add failed: {}", String::from_utf8_lossy(&add.stderr));

    let list = Command::new(env!("CARGO_BIN_EXE_xscore"))
        .args(["link", "config", "list"]).output().unwrap();
    let s = String::from_utf8_lossy(&list.stdout);
    assert!(s.contains("web"), "expected alias 'web' in config list, got: {s}");

    let _ = Command::new(env!("CARGO_BIN_EXE_xscore"))
        .args(["link", "config", "remove", "web"]).output();
}
```
```

**Step 2: README**

```markdown
# xscore

Rust session manager for SSH / shell / serial, modeled as **Link x Pane**.

- **Link** = one physical connection (SSH channel, serial port, local child, or tmux server).
- **Pane** = one working view into a link. N panes per link; each pane has its own id, snapshot, write state.

## Quick start

\`\`\`
xscore link add shell:///bin/bash
xscore link add ssh://user@host:22
xscore link add serial:///dev/ttyUSB0?baud=115200

# Persistent template (one-time):
xscore link config add web ssh://u@h:22?identity=~/.ssh/id_ed25519

# Reuse template + attach multiple panes on the same link:
xscore link add ssh://u@h:22 -c web -a web1
xscore link add ssh://u@h:22 --link=existing -a web2
\`\`\`

## Commands

\`\`\`
xscore link add <url> [-c CONFIG] [-a ALIAS] [--link=existing]
xscore link open --id <link-id>
xscore link close --id <link-id>
xscore link remove --id <link-id>
xscore link show --id <link-id>
xscore link list

xscore link config add <alias> <url>
xscore link config remove <alias>
xscore link config show <alias>
xscore link config list

xscore pane add --link <link-id> [--name=NAME]
xscore pane open --id <pane-id> [--link <link-id>]
xscore pane close --id <pane-id>
xscore pane remove --id <pane-id>
xscore pane show --id <pane-id>
xscore pane list [--link <link-id>]

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
       +-- LinkRegistry     (by_id, by_address, by_alias)
       |    +-- Link { transport + sink + Vec<PaneId> }
       +-- PaneRegistry     (pane_id -> PaneInfo)
       +-- BackendRegistry  (pane_id -> Arc<dyn PaneBackend>)
                                  ^ wraps (Link, sink_handle)
\`\`\`

Each Link owns one Transport (raw IO) + one Sink (PTY/ANSI/tmux). Each Pane lives inside
one link and addresses the link's sink via a sink_handle (e.g. tmux `%42`). Closing a
pane does not close the link; closing a link drops every pane on it.

**`add` vs `open`:** `add` creates a new external resource; `open` re-attaches an
existing one. **`close` vs `remove`:** `close` kills the underlying resource (transport
+ sink + tmux window); `remove` only unregisters from in-memory state and leaves the
external resources alive.

`--link=existing` opts into **address-keyed dedup**: search LinkRegistry for an existing
link matching the URL's transport address; if found, open a new pane on it; if not found,
error. Without `--link=existing`, every `xscore link add` creates a brand-new link.
```

**Step 4: Run all tests**

Run: `cargo test -p xscore`
Expected: all unit + integration tests pass.

**Step 5: Commit**

```bash
git add tests/ README.md
git commit -m "test+docs: Link x Pane smoke tests + README"
```