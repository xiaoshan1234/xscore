# xscore

Rust session manager for SSH / shell / serial, modeled as three **independent entities** — Link, Pane, Window — wired together by Option<> associations on Pane (ADR 0007 v2, v0.3.0).

- **Link** = one physical connection (SSH channel, serial port, local child, or one tmux session inside the shared `tmux -L xscore` server).
- **Pane** = one working buffer (`PaneBridge`) + UI info (cols/rows/x/y + which link + which window). Pane is the unit of `data read/write/resize`.
- **Window** = one pure UI layout container (layout + pane list + total cols/rows). Independent of links.

> **Default behavior (post v0.2.0):** A fresh `xscore link add <url>` runs the transport directly — no tmux server in the loop. To get the full Link × Window × Pane tree plus data ops, opt in to tmux with `?tmux=on` (or `?tmux=auto|local|remote`) in the URL — see [Sink options](#sink-options-tmux-opt-in).

> **v0.3.0 redesign:** `xscore link add` creates ONLY the link — no auto window/pane. Panes and windows are independent entities; you wire them together via `pane attach`, `pane add-to-window`, etc. See [5-minute demo](#5-minute-demo) and [ADR 0007](doc/dev/adr/0007-raw-default-tmux-opt-in.md).

This is a **Rust port of the xst/script patterns** (without the xst dependency): a single CLI that orchestrates a `~/.config/xscore/` tree for persistent state. Tmux is one option, not the default.

## 5-minute demo

```bash
# 1. Build (one-time). Rust 1.95+, tokio runtime.
cargo build --release

# 2. Add a link. By default this is a Raw link: the transport runs
#    directly with no tmux server, so only `link:` is printed.
#    v0.3.0: NO auto window/pane. The link is standalone.
./target/release/xscore link add shell:///bin/bash
# stdout:
#   link:  01928e0a-7d10-7000-8000-000000000001

# 2b. (Optional) opt in to the full Link × Window × Pane tree by
#     using a tmux-backed link — still only `link:` is printed
#     in v0.3.0 (no auto window/pane).
./target/release/xscore link add 'shell:///bin/bash?tmux=on'
# stdout:
#   link:  01928e0a-7d10-7000-8000-000000000002

# 3. Add an orphan pane (no link, no window).
./target/release/xscore pane add
# stdout:
#   pane:  01928e0a-7d10-7000-8000-000000000003

# 4. Attach the pane to the link — creates the bridge
#    (DirectBridge for raw links, TmuxBridge for tmux links).
#    v0.3.0 Q1: for raw links, the previously-active pane (if any)
#    is auto-detached (its link_id becomes null; buffer preserved).
./target/release/xscore pane attach --pane <pid> --link <lid>
# stdout:
#   attached: 01928e0a-...

# 5. Data I/O via the bridge. Errors with NotAttached(pid) for
#    orphan panes.
echo 'echo hi from xscore' | ./target/release/xscore data write --pane <pid>
./target/release/xscore data read --pane <pid>

# 6. Add a window (orphan — no link required).
./target/release/xscore window add --cols 120 --rows 30
# stdout:
#   window: 01928e0a-...

# 7. Put the pane in the window.
./target/release/xscore pane add-to-window --pane <pid> --window <wid>

# 8. Verify state (persisted to ~/.config/xscore/state/state.json
#    automatically after every mutating call).
./target/release/xscore link list
./target/release/xscore pane show --id <pid>    # shows link_id, window_id, geometry
./target/release/xscore window list             # shows panes in each window
./target/release/xscore pane list --window <wid>

# 9. Detach the pane (buffer preserved; re-attach later to the
#    same or different link).
./target/release/xscore pane detach --pane <pid>

# 10. Clean up.
./target/release/xscore pane del --id <pid>
./target/release/xscore window del --id <wid>
./target/release/xscore link del --id <lid>
```

## The 3-entity model

xscore exposes three independent entities. They are NOT a hierarchy —
each is orphan-creatable, and associations are dynamic Option IDs on
`PaneInfo`:

```
Link ────owns───> Transport (Arc<dyn>) + Sink (Box<dyn>)
       ──tmux_sink──> Option<Arc<TmuxSink>>         (for TmuxBridge)
       ──active_pane──> Mutex<Option<PaneId>>       (Q1 cache)

Pane ────bridge───> Arc<dyn PaneBridge>             (runtime, NOT persisted)
        info: { cols, rows, x, y,                    (Q2 geometry)
                link_id: Option<LinkId>,             (None = orphan)
                window_id: Option<WindowId> }         (None = orphan)

Window ──panes───> Mutex<Vec<PaneId>>                (UI parent)
        info: { layout: WindowLayout,
                cols, rows }                          (total window size)
```

Panes can move freely:

- `pane attach --pane <pid> --link <lid>` — bind pane to link
- `pane detach --pane <pid>` — unbind (buffer preserved)
- `pane add-to-window --pane <pid> --window <wid>` — put in a window
- `pane remove-from-window --pane <pid> --window <wid>` — remove from window

## Q1: Direct link 1-active-pane rule

For **shell/ssh/serial links** (the default — `RawSink`), only one
pane may be active at a time. When a new pane is attached, the
previously-active pane is auto-detached (its `info.link_id` becomes
`None`, its buffer is preserved). This is the user-confirmed Q1 rule
(2026-10-19).

For **tmux-backed links** (`?tmux=on|auto|local|remote`), multiple
panes may attach simultaneously — each `attach_pane` creates a new
tmux split-window in the link's session.

## Q2: Pane geometry

A pane's UI info is geometry — `cols`, `rows`, `x`, `y` (user-confirmed
Q2, 2026-10-19). Each pane declares its own size and position
independently of its parent window. The window's own `cols`/`rows` are
the total window size.

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
- `tmux` 3.0+ on `$PATH` (only needed for `?tmux=on` paths)
- A real serial device or SSH server for the non-shell transports (optional; shell is the test path)

## Commands

```
xscore link add <url?> [-c CONFIG] [--id <uuid>]      # create new (url OR -c CONFIG required)
xscore link del --id <link-id>                       # full teardown + cascades pane detach
xscore link open <url?> [-c CONFIG]                  # attach existing tmux session (tmux only)
xscore link close --id <link-id>                     # tmux-detach only (session keeps running)
xscore link show --id <link-id>
xscore link list

xscore link config add <name> <url>                   # save a ConnectSpec template
xscore link config remove <name>
xscore link config show <name>
xscore link config list

xscore window add [--name N] [--layout <kind>] [--cols C --rows R]
xscore window del --id <wid>
xscore window list
xscore window show --id <wid>
xscore window set-layout --id <wid> --layout <kind>
xscore window add-pane --window <wid> --pane <pid>
xscore window remove-pane --window <wid> --pane <pid>

xscore pane add [--name N] [--cols C --rows R] [--x X --y Y]
xscore pane del --id <pid>
xscore pane show --id <pid>
xscore pane list [--window <wid>] [--link <lid>]
xscore pane attach --pane <pid> --link <lid>          # Q1: auto-detach for raw links
xscore pane detach --pane <pid>
xscore pane resize --pane <pid> --cols C --rows R
xscore pane move --pane <pid> --x X --y Y             # geometry only (no bridge)
xscore pane add-to-window --pane <pid> --window <wid>
xscore pane remove-from-window --pane <pid> --window <wid>

xscore data read --pane <pid> [--bytes N]            # errors with NotAttached for orphans
xscore data write --pane <pid> [< file | --from-file F]
xscore data resize --pane <pid> --cols C --rows R

xscore config show                                   # stub for v0.3.0
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

A link has a `sink` that controls how its I/O is mediated. The default is **`raw`** — the transport is the I/O source directly, no tmux server involved. Opt in to tmux to get multi-pane capability plus `xscore data {read, write, resize}` (which need the bridge — see [ADR 0007](doc/dev/adr/0007-raw-default-tmux-opt-in.md)).

| `?tmux=` value | Sink | Multi-pane | Q1 (1-active-pane) |
|---|---|---|---|
| _absent_ (default) | `raw` | no | yes (auto-detach on new attach) |
| `?tmux=off` | `raw` | no | yes |
| `?tmux=on` | `tmux` (mode=auto) | yes | no |
| `?tmux=auto` | `tmux` (mode=auto) | yes | no |
| `?tmux=local` | `tmux` (mode=local) | yes | no |
| `?tmux=remote` | `tmux` (mode=remote) | yes | no |

`auto` resolves at link-creation time: `ssh://` → `remote` (tmux on the remote host), `shell://` / `serial://` → `local` (the local shared `tmux -L xscore` server). `name=X` overrides the auto-generated `xscore-<link_id>` tmux session name.

**Why default to raw?** Most users just want a clean CLI for SSH / shell / serial — the tmux server is overhead they don't need. Opt-in to tmux when you want multi-pane capability (Q1 corollary) or the window/pane tree.

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

## Architecture

```
CLI -> SessionManager
       +-- LinkRegistry     (link_id → Arc<Link>)
       +-- WindowRegistry   (window_id → Arc<Window>)         — independent
       +-- PaneRegistry     (pane_id → PaneInfo)              — orphan-creatable
       +-- pane_bridges     (pane_id → Arc<dyn PaneBridge>)   — built on attach_pane
                                  ▲ wraps:
                                   ├─ DirectBridge { Arc<dyn Transport> }    (raw)
                                   └─ TmuxBridge { Weak<TmuxSink>, handle } (tmux)
```

Each Link owns one Transport (raw IO) + one Sink (PTY/ANSI/tmux or no-op). Each Pane has its own bridge (the runtime I/O channel) and its own geometry (cols/rows/x/y). Each Window owns its pane list (UI container).

**Lifecycle:**
- `link del` cascades: every attached pane's `link_id` is cleared, bridge closed silently. Panes survive as orphans.
- `window del` cascades: every pane in the window has its `window_id` cleared. Panes survive as orphans.
- `pane del` fully tears down: also detaches from link + removes from window.

**`add` vs `del` vs `open` vs `close`** (ADR 0005, four-verb `link` tree):
- `add` creates a brand-new external resource: transport + sink. (v0.3.0: NO auto window/pane.)
- `del` is the universal full teardown: cascades pane detach + kills sink + transport. For tmux: kills the per-link session; the shared `-L xscore` server stays up.
- `open` attaches to an existing tmux session (tmux only — shell/serial/ssh return `OpenOnlyForTmux`).
- `close` is a no-op for v0.3.0 (tmux sessions are inherently persistent across xscore invocations).

## Status (v0.3.0)

**Working end-to-end:**
- 3-entity decoupled model with dynamic associations (ADR 0007 v2)
- Q1: direct-link 1-active-pane auto-detach (user-confirmed 2026-10-19)
- Q1 corollary: tmux-backed multi-pane (each attach = tmux split-window)
- Q2: pane geometry (cols/rows/x/y) (user-confirmed 2026-10-19)
- Bridge model: `DirectBridge` (raw) + `TmuxBridge` (tmux) via `PaneBridge` trait
- Local tmux sink: `tmux new-session / split-window / kill-pane / capture-pane / send-keys / resize-pane` over the shared `-L xscore` server (ADR 0003)
- `shell://` transport (local child, real `tokio::process::Command`)
- URL parser: `shell://` / `ssh://` / `serial://` with `?key=value` query
- Persistent ConnectSpec templates: `xscore link config {add,remove,show,list}` at `~/.config/xscore/links/`
- Per-pane data ops: `xscore data {read, write, resize}` via bridge
- 4-verb trees for `link` / `window` / `pane` (ADR 0005)
- State persistence across CLI invocations (`~/.config/xscore/state/state.json`, **flat v0.3.0 schema**)
- **Default sink = `raw` (no tmux)**; opt in to tmux with `?tmux=on` or `?tmux=auto|local|remote` in the URL

**Implemented but not exercised by the test suite:**
- `serial://` transport (real `serialport = "4.10"` impl) — needs real hardware
- `ssh://` transport (real `russh = "0.43"` impl) — needs a real SSH server; key auth only (no `known_hosts`, no password)

**Stubbed / pending (deferred to v0.4.0+):**
- `link open` for the local-tmux case is a `todo!()` stub
- `xscore config {show, path, set, unset}` for AppConfig
- `link config edit` for in-place template editing
- Pre-built binaries (currently install via `cargo install --path .`)

**Known limitations (deferred):**
- Hydration no longer auto-builds bridges — data ops on a link that survives a process restart fail with `NotAttached(pid)` until the user runs `xscore pane attach` again. To resume I/O after restart: `xscore pane attach --pane <pid> --link <lid>`.
- `xscore link open` for tmux is `todo!()` — will land in v0.4.0.
- SSH `check_server_key` accepts any host key. Real users want `StrictHostKeyChecking=yes` with `~/.ssh/known_hosts`.
- SSH auth is key-only (no password field in `ConnectSpec::Ssh` yet).
- Serial `default-features = false` (no `libudev`); open by explicit path.
- Local ring buffer for direct bridges (deferred to v0.4.0+).

## Tests

```bash
# Unit tests (20 tests, in-process, <1s)
cargo test -p xscore --lib

# CLI integration tests (13 tests, ~1s)
cargo test -p xscore --test cli_smoke

# Data-path integration tests (5 tests, single-process, real tmux round-trip)
cargo test -p xscore --test data_smoke

# Everything (38 tests total)
cargo test -p xscore
```

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
    state.rs                     # save / load (flat StateSnapshot)
  backends/                     # PaneBridge trait + DirectBridge + TmuxBridge
    bridge.rs
  model/                        # ConnectSpec, TransportConfig, SinkConfig,
                                #   LinkInfo, WindowInfo, WindowLayout, PaneInfo
    session.rs
  transport/                    # Transport trait + 3 impls + factory
    mod.rs
    factory.rs
    shell.rs, serial.rs, ssh.rs
  sink/                         # Sink trait + 2 impls + factory
    mod.rs
    factory.rs
    raw.rs, tmux.rs
  url/                          # URL parser
    connect.rs
  util/                          # tmux_arg_socket() helper
    tmux_socket.rs
  cli/                          # clap 4-level subcommand tree
    mod.rs
    link.rs, window.rs, pane.rs, data.rs
  config/                       # link templates on disk
    mod.rs, paths.rs, loader.rs

crates/xscore/tests/
  cli_smoke.rs                  # 13 CLI integration tests
  data_smoke.rs                  # 5 in-process data-path tests
```

## Release process

See [`RELEASE.md`](RELEASE.md) for the v0.1.0 release checklist and version-bump policy.

## License

MIT (see `LICENSE`).