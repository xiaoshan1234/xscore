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

    // Second add with same URL → second, distinct link (ADR 0005: no --link=existing; every `add` creates new).
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
fn xscore_link_open_unsupported_for_shell() {
    // ADR 0005: `link open` is tmux-only; shell/serial/ssh return OpenOnlyForTmux.
    let out = Command::new(env!("CARGO_BIN_EXE_xscore"))
        .args(["link", "open", "shell:///bin/cat"]).output().unwrap();
    assert!(!out.status.success(), "shell + `link open` must fail");
    let s = String::from_utf8_lossy(&out.stderr);
    assert!(s.contains("tmux") || s.contains("OpenOnlyForTmux"),
            "expected OpenOnlyForTmux error, got: {s}");
}

#[test]
fn xscore_link_close_unsupported_for_shell() {
    // ADR 0005: `link close` is tmux-only; shell/serial/ssh return CloseOnlyForTmux.
    let out = Command::new(env!("CARGO_BIN_EXE_xscore"))
        .args(["link", "add", "shell:///bin/cat"]).output().unwrap();
    let link_line = String::from_utf8_lossy(&out.stdout).lines()
        .find(|l| l.starts_with("link:")).unwrap().to_string();
    let link_id = link_line.trim().strip_prefix("link:").unwrap().trim().to_string();

    let close = Command::new(env!("CARGO_BIN_EXE_xscore"))
        .args(["link", "close", "--id", &link_id]).output().unwrap();
    assert!(!close.status.success(), "shell + `link close` must fail");
    let s = String::from_utf8_lossy(&close.stderr);
    assert!(s.contains("tmux") || s.contains("CloseOnlyForTmux"),
            "expected CloseOnlyForTmux error, got: {s}");

    // cleanup via `link del`
    let _ = Command::new(env!("CARGO_BIN_EXE_xscore"))
        .args(["link", "del", "--id", &link_id]).output();
}

#[test]
fn xscore_link_add_requires_url_or_config() {
    // ADR 0005: at least one of <url> or -c CONFIG is required.
    let out = Command::new(env!("CARGO_BIN_EXE_xscore"))
        .args(["link", "add"]).output().unwrap();
    assert!(!out.status.success(), "`link add` with no inputs must fail");
}

#[test]
fn xscore_link_del_kills_link() {
    let add = Command::new(env!("CARGO_BIN_EXE_xscore"))
        .args(["link", "add", "shell:///bin/cat"]).output().unwrap();
    let link_id = String::from_utf8_lossy(&add.stdout).lines()
        .find(|l| l.starts_with("link:")).unwrap().trim().strip_prefix("link:").unwrap().trim().to_string();

    let del = Command::new(env!("CARGO_BIN_EXE_xscore"))
        .args(["link", "del", "--id", &link_id]).output().unwrap();
    assert!(del.status.success(), "link del failed: {}", String::from_utf8_lossy(&del.stderr));

    // After del, the link must be gone from `link list`.
    let list = Command::new(env!("CARGO_BIN_EXE_xscore"))
        .args(["link", "list"]).output().unwrap();
    assert!(!String::from_utf8_lossy(&list.stdout).contains(&link_id),
            "link still in registry after del: {link_id}");
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
    assert!(s.contains("web"), "expected name 'web' in config list, got: {s}");

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
xscore link add ssh://u@h:22 -c web           # 1st pane on the link

# Open / detach / destroy — verb split (ADR 0005):
xscore link open 'tmux://my-session'           # attach existing tmux session
xscore link close --id <link-id>               # tmux-detach only (session keeps running)
xscore link del --id <link-id>                 # full teardown (kills tmux server)
\`\`\`

## Commands

\`\`\`
xscore link add <url?> [-c CONFIG]                     # always create new
xscore link del --id <link-id>                            # full teardown (kills transport + sink + tmux server)
xscore link open <url?> [-c CONFIG]                       # attach existing tmux session (tmux only)
xscore link close --id <link-id>                          # detach (tmux only; session keeps running)
xscore link show --id <link-id>
xscore link list

xscore link config add <name> <url>
xscore link config remove <name>
xscore link config show <name>
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
       +-- LinkRegistry     (by_id, by_address)
       |    +-- Link { transport + sink + Vec<PaneId> }
       +-- PaneRegistry     (pane_id -> PaneInfo)
       +-- BackendRegistry  (pane_id -> Arc<dyn PaneBackend>)
                                  ^ wraps (Link, sink_handle)
\`\`\`

Each Link owns one Transport (raw IO) + one Sink (PTY/ANSI/tmux). Each Pane lives inside
one link and addresses the link's sink via a sink_handle (e.g. tmux `%42`). Closing a
pane does not close the link; closing a link drops every pane on it.

**`add` vs `del` vs `open` vs `close`** (ADR 0005, four-verb `link` tree):
- `add` creates a brand-new external resource (transport + sink + 1st pane).
- `del` is the universal full teardown: kill transport + sink + every pane, plus (for tmux) the per-link tmux server.
- `open` attaches to an existing tmux session (tmux only — shell/serial/ssh return `OpenOnlyForTmux`).
- `close` is the in-memory detach: unregister the link and its panes, but the tmux session keeps running (tmux only — shell/serial/ssh return `CloseOnlyForTmux`).

`link open` and `link close` are tmux-only because shell/serial/ssh processes don't have cross-invocation identity to attach to or detach from. `link add <same url>` always creates a brand-new link; to re-attach to an existing resource, the verb is `link open`.
```

**Step 4: Run all tests**

Run: `cargo test -p xscore`
Expected: all unit + integration tests pass.

**Step 5: Commit**

```bash
git add tests/ README.md
git commit -m "test+docs: Link x Pane smoke tests + README"
```