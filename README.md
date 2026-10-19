# xscore

Rust session manager for SSH / shell / serial, modeled as **Link × Window × Pane** (ADR 0006).

- **Link** = one physical connection (SSH channel, serial port, local child, or one tmux session inside the shared `tmux -L xscore` server).
- **Window** = one tmux window inside the link's session. A link can have N named windows. (Tmux-backed links only.)
- **Pane** = one working view — a tmux split-window inside a Window. A window can have M panes sharing its screen. (Tmux-backed links only.)

> **Default behavior (post v0.2.0):** A fresh `xscore link add <url>` runs the transport
> directly — no tmux server in the loop. The `xscore window {add,del,list,show}` and
> `xscore pane {add,del,list,show}` subcommands are only valid for tmux-backed links.
> To get the full Link × Window × Pane tree, opt in to tmux with `?tmux=on` (or
> `?tmux=auto|local|remote`) in the URL — see [Sink options](#sink-options-tmux-opt-in).

This is a **Rust port of the xst/script patterns** (without the xst dependency): a single CLI that orchestrates a `~/.config/xscore/` tree for persistent state. Tmux is one option, not the default.

## 5-minute demo

```bash
# 1. Build (one-time). Rust 1.95+, tokio runtime. The build pulls in
#    serialport, russh, clap, etc. — see Cargo.lock for the full list.
cargo build --release

# 2. Add a link. By default this is a Raw link: the transport runs
#    directly with no tmux server, so only `link:` is printed.
./target/release/xscore link add shell:///bin/bash
# stdout:
#   link:  01928e0a-7d10-7000-8000-000000000001

# 2b. (Optional) opt in to the full Link × Window × Pane tree:
./target/release/xscore link add 'shell:///bin/bash?tmux=on'
# stdout:
#   link:  01928e0a-7d10-7000-8000-000000000002
#   window:01928e0a-7d10-7000-8000-000000000003
#   pane:  01928e0a-7d10-7000-8000-000000000004

# 3. List links (state is persisted to ~/.config/xscore/state/state.json
#    automatically after every mutating call).
./target/release/xscore link list
# stdout:
#   LINK ID                               NAME
#   01928e0a-...                         01928e0a-...
#   01928e0a-...                         01928e0a-...

# 4. (Tmux-backed link only) add another window to the same link.
./target/release/xscore window add --link <tmux-link-id> --name dev
# stdout:
#   window:01928e0a-7d10-7000-8000-000000000005

# 5. (Tmux-backed link only) add a split-pane to the dev window.
./target/release/xscore pane add --window <window-id-2> --name bash
# stdout:
#   pane:  01928e0a-7d10-7000-8000-000000000006

# 6. (Tmux-backed link only) read the visible content of a pane
#    (uses tmux capture-pane).
./target/release/xscore data read --pane <pane-id> --bytes 4096

# 7. (Tmux-backed link only) write text into a pane (uses tmux send-keys).
echo -n 'echo hi from xscore' | ./target/release/xscore data write --pane <pane-id>

# 8. Verify state survives process exit (in another shell):
./target/release/xscore link list
# stdout: same link ids as step 3
./target/release/xscore window list --link <tmux-link-id>
# stdout: both window ids from steps 2b + 4

# 9. Clean up.
./target/release/xscore link del --id <link-id>
```

## Installation

```bash
# From the workspace root:
cargo install --path .

# Or clone and run from source:
git clone <repo> xscore && cd xscore
cargo build --release
./target/release/xscore --help
```

**Requirements:**
- Rust 1.95+
- `tmux` 3.0+ on `$PATH` (the `xscore` link tree is built around tmux's session/window/pane model)
- A real serial device or SSH server for the non-shell transports (optional; shell is the test path)

## Commands

```
xscore link add <url?> [-c CONFIG]            # always create new (url OR -c CONFIG required)
xscore link del --id <link-id>                # full teardown (kills transport + sink + the per-link tmux session)
xscore link open <url?> [-c CONFIG]           # attach existing tmux session (tmux only; non-tmux errors)
xscore link close --id <link-id>              # tmux-detach only (session keeps running; non-tmux errors)
xscore link show --id <link-id>
xscore link list

xscore link config add <name> <url>            # save a ConnectSpec template to ~/.config/xscore/links/
xscore link config remove <name>
xscore link config show <name>
xscore link config list

xscore window add   --link <link-id> [--name=NAME]
xscore window list  [--link <link-id>]
xscore window show  --id <window-id>
xscore window del   --id <window-id>

xscore pane add --window <window-id> [--name=NAME]
xscore pane del --id <pane-id>
xscore pane show --id <pane-id>
xscore pane list [--window <window-id>]

xscore data read --pane <pane-id> [--bytes N]
xscore data write --pane <pane-id> [--from-file FILE]
xscore data resize --pane <pane-id> --cols C --rows R

xscore config show                            # stub for v0.2.0
xscore config path
xscore config set <key> <value>
xscore config unset <key>
```

### URL syntax

| Scheme | Form | Example |
|---|---|---|
| `shell://` | Local child process | `shell:///bin/bash`, `shell:///bin/cat` |
| `serial://` | Serial port (needs `?baud=...`) | `serial:///dev/ttyUSB0?baud=115200&data_bits=8&parity=n&stop_bits=1` |
| `ssh://` | SSH (key-based auth, needs `?identity=...`) | `ssh://user@host:22?identity=~/.ssh/id_ed25519` |

### Sink options (tmux opt-in)

A link has a `sink` that controls how its I/O is mediated. The default is **`raw`** — the transport is the I/O source directly, no tmux server involved. Opt in to tmux to get the full Link × Window × Pane tree plus `xscore data {read, write, resize}`.

| `?tmux=` value | Sink | Window/Pane tree | Data ops |
|---|---|---|---|
| _absent_ (default) | `raw` | no | (no panes) |
| `?tmux=off` | `raw` | no | (no panes) |
| `?tmux=on` | `tmux` (mode=auto) | yes | yes |
| `?tmux=auto` | `tmux` (mode=auto) | yes | yes |
| `?tmux=local` | `tmux` (mode=local) | yes | yes |
| `?tmux=remote` | `tmux` (mode=remote) | yes | yes |

`auto` resolves at link-creation time: `ssh://` → `remote` (tmux on the remote host), `shell://` / `serial://` → `local` (the local shared `tmux -L xscore` server). `name=X` overrides the auto-generated `xscore-<link_id>` tmux session name.

**Why default to raw?** xscore was originally built to manage tmux sessions, but most users just want a clean CLI for SSH / shell / serial — the tmux server is overhead they don't need. Opt-in to tmux when you want session persistence, the window/pane tree, or `xscore data` ops.

### Link id (specify at creation)

`xscore link add <url> [--id <uuid>]` lets the caller supply a specific `LinkId` (uuid v7) instead of auto-generating one. Useful for scripted workflows where the id must be known in advance (e.g., for reconnect-style flows or deterministic tests).

```bash
# Default: auto-generated uuid v7
$ xscore link add shell:///bin/cat
link:  01928e0a-7d10-7000-8000-000000000001

# Specify a known id
$ xscore link add shell:///bin/cat --id 11111111-2222-7333-8444-555555555555
link:  11111111-2222-7333-8444-555555555555

# Duplicate id → error
$ xscore link add shell:///bin/echo --id 11111111-2222-7333-8444-555555555555
Error: link id already in use: 11111111-2222-7333-8444-555555555555
```

The `--id` value must be a valid uuid (any version; v7-shaped is recommended for forward-compat with chronological ordering). The id is checked for collision against the in-memory `LinkRegistry` — if the id is already in use, `Error::LinkIdExists` is returned.

## Architecture

```
CLI -> SessionManager
       +-- LinkRegistry     (by_id, by_address)         -- one Link per physical connection
       +-- WindowRegistry   (by_id, by_link)           -- one Window per tmux window (ADR 0006)
       +-- PaneRegistry     (pane_id -> PaneInfo;     -- one Pane per tmux split-window
                              PaneInfo.window_id)       (ADR 0006: was PaneInfo.link_id)
       +-- BackendRegistry  (pane_id -> Arc<dyn PaneBackend>)
                                  ^ wraps (Link, sink_handle)
```

Each Link owns one Transport (raw IO) + one Sink (PTY/ANSI/tmux). Each Window inside a link is a tmux window inside the link's session. Each Pane inside a window is a tmux split-window sharing the window's screen. Killing a pane does not affect its siblings or its parent window; killing a window kills every pane inside it (via `tmux kill-window`); closing a link keeps the tmux session alive (re-attachable via `link open`); killing a link (`link del`) drops every window + pane and the tmux session.

**`add` vs `del` vs `open` vs `close`** (ADR 0005, four-verb `link` tree):
- `add` creates a brand-new external resource: transport + sink + a default window + a default pane.
- `del` is the universal full teardown: kill transport + sink + every window + every pane, plus (for tmux) the per-link tmux session. The shared `tmux -L xscore` server stays up.
- `open` attaches to an existing tmux session (tmux only — shell/serial/ssh return `OpenOnlyForTmux`).
- `close` is the in-memory detach: unregister the link + windows + panes, but the tmux session keeps running (tmux only — shell/serial/ssh return `CloseOnlyForTmux`).

`link open` and `link close` are tmux-only because shell/serial/ssh processes don't have cross-invocation identity to attach to or detach from. `link add <same url>` always creates a brand-new link; to re-attach to an existing resource, the verb is `link open`. See [ADR 0005](doc/dev/adr/0005-add-open-close-del-verbs.md) for the rationale.

## Status (v0.2.0-dev)

**Working end-to-end:**
- 3-tier model: Link × Window × Pane (ADR 0006) wired (tmux-backed links only)
- Local tmux sink: `tmux new-session / new-window / split-window / kill-*` over the shared `-L xscore` server (ADR 0003)
- `shell://` transport (local child, real `tokio::process::Command`)
- URL parser: `shell://` / `ssh://` / `serial://` with `?key=value` query
- Persistent ConnectSpec templates: `xscore link config {add,remove,show,list}` at `~/.config/xscore/links/`
- Per-pane data ops: `xscore data {read, write, resize}` over tmux's `capture-pane` / `send-keys` / `resize-pane` (tmux-backed links only)
- 4-verb trees for `link` / `window` / `pane` (ADR 0005)
- ADR 0005 invariants: `link open` and `link close` reject non-tmux transports with `OpenOnlyForTmux` / `CloseOnlyForTmux`
- State persistence across CLI invocations (`~/.config/xscore/state/state.json`)
- **Default sink = `raw` (no tmux)**; opt in to tmux with `?tmux=on` or `?tmux=auto|local|remote` in the URL — see [Sink options](#sink-options-tmux-opt-in)

**Implemented but not exercised by the test suite:**
- `serial://` transport (real `serialport = "4.10"` impl) — needs real hardware
- `ssh://` transport (real `russh = "0.43"` impl) — needs a real SSH server; key auth only (no `known_hosts`, no password)

**Stubbed / pending (deferred to v0.2.0+):**
- `link open` for the local-tmux case is a `todo!()` stub (manager.rs `open_link`); non-tmux correctly returns `OpenOnlyForTmux`
- `xscore config {show, path, set, unset}` (top-level config — separate from `xscore link config`)
- `link config edit` for in-place template editing
- Pre-built binaries (currently install via `cargo install --path .`)

**Known limitations (deferred):**
- Hydration uses `NoopTransport` — `data read` / `data write` on a link that survives a process restart fail (the original child process / SSH channel is gone). To re-attach I/O, run `xscore link add <url>` again (creates a NEW link with a fresh transport) or `xscore link open` once that lands.
- `xscore link open` for tmux is `todo!()` — will land in v0.2.0.
- SSH `check_server_key` accepts any host key. Real users want `StrictHostKeyChecking=yes` with a `~/.ssh/known_hosts` lookup.
- SSH auth is key-only (no password field in `ConnectSpec::Ssh` yet).
- Serial `default-features = false` (no `libudev`); open by explicit path. Future `xscore link serial list` would need `libudev-dev` or `/sys/class/tty/*` scanning.

## Tests

```bash
# Unit tests (20 tests, in-process, <1s)
cargo test -p xscore --lib

# CLI integration tests (14 tests, ~1s, requires live tmux for `?tmux=...` paths)
cargo test -p xscore --test cli_smoke

# Data-path integration tests (3 tests, single-process, real tmux round-trip)
cargo test -p xscore --test data_smoke

# Everything (37 tests total)
cargo test -p xscore
```

**Coverage map:**

| Area | Where | Count |
|---|---|---|
| URL parsing (incl. sink opt-in) | `url::connect::tests` | 11 |
| Core id roundtrips + distinctness | `core::id::tests` | 3 |
| `LinkAddress` equality | `core::address::tests` | 2 |
| `Transport` trait object-safety | `transport::tests` (echo) | 1 |
| `ShellTransport` cat round-trip | `transport::shell::tests` | 1 |
| `Sink` trait object-safety | `sink::tests` (noop) | 1 |
| CLI help tree | `cli_smoke` | 1 |
| CLI input validation (link add needs url) | `cli_smoke` | 1 |
| ADR 0005 invariant (link open non-tmux) | `cli_smoke` | 1 |
| Disk-based config templates (link config add/list) | `cli_smoke` | 1 |
| Two `link add <same url>` = two distinct ids (ADR 0005) | `cli_smoke` | 1 |
| Multi-invocation state (add → list → del_pane → del_link) | `cli_smoke` | 1 |
| ADR 0005 invariant (link close non-tmux) | `cli_smoke` | 1 |
| `link del` removes from list | `cli_smoke` | 1 |
| `window add` inside link, visible in `window list` | `cli_smoke` | 1 |
| `window del` removes from list | `cli_smoke` | 1 |
| `pane add --window` (not `--link`) — ADR 0006 | `cli_smoke` | 1 |
| **Default sink = raw (no window/pane printed)** | `cli_smoke` | 1 |
| **`window add` errors on raw link (NotTmuxLink)** | `cli_smoke` | 1 |
| **`?tmux=on` opt-in still creates window+pane** | `cli_smoke` | 1 |
| In-process add → write → snapshot (real tmux) | `data_smoke` | 1 |
| Double-default-window fix (1 link → 1 window) | `data_smoke` | 1 |
| Cross-manager hydration (manager A add → manager B hydrates) | `data_smoke` | 1 |

Total: **37 tests, 0 ignored.**

## File layout

```
crates/xscore/src/
  main.rs                       # binary entry — clap dispatch
  lib.rs                        # module declarations
  core/                         # LinkId, WindowId, PaneId, LinkAddress, Error,
                                #   Link/Window/Pane structs, registries,
                                #   SessionManager, state persistence
    id.rs
    address.rs
    error.rs
    link.rs, pane.rs, window.rs
    link_registry.rs, pane_registry.rs, window_registry.rs
    manager.rs                   # SessionManager
    state.rs                     # save / load
  backends/                     # PaneBackend trait + BackendRegistry + CompositeBackend
  model/                        # ConnectSpec, TransportConfig, SinkConfig,
                                #   LinkInfo, WindowInfo, PaneInfo
    session.rs
  transport/                    # Transport trait + 3 impls + factory
    mod.rs
    factory.rs
    shell.rs, serial.rs, ssh.rs
  sink/                         # Sink trait + 2 impls + factory + util
    mod.rs
    factory.rs
    raw.rs, tmux.rs
  url/                          # URL parser
    connect.rs
  util/                          # tmux_arg_socket() helper
    tmux_socket.rs
  cli/                          # clap 3-level subcommand tree
    mod.rs
    link.rs, window.rs, pane.rs, data.rs
  config/                       # link templates on disk
    mod.rs, paths.rs, loader.rs

crates/xscore/tests/
  cli_smoke.rs                  # 11 CLI integration tests
  data_smoke.rs                  # 3 in-process data-path tests
```

## Release process

See [`RELEASE.md`](RELEASE.md) for the v0.1.0 release checklist and version-bump policy.

## License

TBD — see [LICENSE](LICENSE) (placeholder; pick at v0.1.0 release time).
