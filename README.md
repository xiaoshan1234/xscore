# xscore

Rust session manager for SSH / shell / serial, modeled as three **independent entities** — Link, Pane, Window — wired together by Option<> associations on Pane (ADR 0007 v2, v0.4.0).

- **Link** = one physical connection (SSH channel, serial port, local child, or one tmux session inside the shared `tmux -L xscore` server).
- **Pane** = one working buffer (`PaneBridge` + ring buffer) + UI info (cols/rows/x/y + which link + which window). Pane is the unit of `data read/write/resize`.
- **Window** = one UI layout container (layout + pane list + total cols/rows), either virtual or a mirror of a real tmux window.

> **Default behavior:** A fresh `xscore link add <url>` runs the transport directly — no tmux server in the loop. It also gives you a window and a pane with a ring buffer, so `data write/read` work immediately. Opt in to tmux with `?tmux=on` (or `?tmux=auto|local|remote`) to get real tmux windows and panes — see [Sink options](#sink-options-tmux-opt-in).

> **v0.4.0:** `xscore link add` returns **three ids** (link, window, pane). tmux links *adopt* the session's `@N` window and `%N` pane instead of guessing; `link add --pane <pid>` binds an existing pane; `link sync` diffs xscore against tmux. See [5-minute demo](#5-minute-demo) and [ADR 0008](doc/dev/adr/0008-tmux-adoption.md).

This is a **Rust port of the xst/script patterns** (without the xst dependency): a single CLI that orchestrates a `~/.config/xscore/` tree for persistent state. Tmux is one option, not the default.

## 5-minute demo

```bash
# 1. Build (one-time). Rust 1.95+, tokio runtime.
cargo build --release

# 2. Add a link. By default this is a direct (raw) link: the transport
#    runs with no tmux server, and you get a virtual window + a pane
#    with a real ring buffer. Three ids are printed.
./target/release/xscore link add shell:///bin/bash
# stdout:
#   link:   01928e0a-7d10-7000-8000-000000000001
#   window: 01928e0a-7d10-7000-8000-000000000002
#   pane:   01928e0a-7d10-7000-8000-000000000003

# 3. Write and read through the pane's buffer (no extra setup).
echo 'echo hi from xscore' | ./target/release/xscore data write --pane <pid>
./target/release/xscore data read --pane <pid>
# hi from xscore

# 4. tmux link: the window and pane are ADOPTED from tmux, so
#    `sink_handle` is a real %N and `tmux_window_handle` a real @N.
./target/release/xscore link add 'shell:///bin/bash?tmux=on'
# stdout:
#   link:   01928e0a-...-0004
#   window: 01928e0a-...-0005
#   pane:   01928e0a-...-0006
./target/release/xscore pane show --id <tmux-pid>    # "sink_handle": "%0"
./target/release/xscore window show --id <tmux-wid>  # "tmux_window_handle": "@0"

# 5. A second window inside that same tmux session, then a pane
#    (a real tmux split) inside that window.
./target/release/xscore window add --name logs --link <tmux-lid>
./target/release/xscore pane add --window <tmux-wid>
# stdout:
#   pane:  01928e0a-...-0007

# 6. Bind a pane you created earlier instead of auto-creating one.
#    (Direct links only; tmux links must let tmux create their panes.)
./target/release/xscore pane add            # hollow orphan pane
./target/release/xscore link add shell:///bin/cat --pane <pid>
# stdout:
#   link:  01928e0a-...-0008
#   pane:  01928e0a-...-0009

# 7. Is tmux still what xscore thinks it is? (read-only, then heal)
./target/release/xscore link sync --id <tmux-lid>
# in sync: registry matches tmux
# ...or, after someone touched tmux directly:
# drift: pane %1 exists in tmux but not in xscore (not adopted)
./target/release/xscore link sync --id <tmux-lid> --heal
# healed: 0 row(s) corrected, 1 removed

# 8. Verify state (persisted to ~/.config/xscore/state/state.json
#    after every mutating call).
./target/release/xscore link list
./target/release/xscore window list             # includes the TMUX column
./target/release/xscore pane list --window <wid>
./target/release/xscore pane show --id <pid>    # link_id, window_id, sink_handle, geometry

# 9. Detach a direct pane (its buffer survives; re-attach resumes it).
./target/release/xscore pane detach --pane <direct-pid>
# detaching a tmux pane is refused: it is tmux's — use `pane del`.

# 10. Clean up. `link del` orphans direct panes but takes tmux panes
#     down with the session.
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
      ──buffer───> Arc<Mutex<RingBuffer>>           (direct panes; survives detach)
        info: { cols, rows, x, y,                    (Q2 geometry; tmux's values)
                link_id: Option<LinkId>,             (None = orphan)
                window_id: Option<WindowId>,         (None = orphan)
                sink_handle: Option<String> }        ("%N" — None for direct panes)

Window ──panes───> Mutex<Vec<PaneId>>                (UI parent)
        info: { layout: WindowLayout,
                cols, rows,                          (total window size)
                tmux_window_handle: Option<String> } ("@N" — None for virtual windows)
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
panes may attach simultaneously — each `pane add --window <wid>` (or
`pane attach`) creates a new tmux split inside that window, and the new
pane takes tmux's `%N` handle and geometry.

## tmux adoption (`@N` / `%N`)

tmux is the source of truth for anything it created (ADR 0008):

- A tmux link's window/pane come from `tmux list-windows` / `list-panes`
  at adoption time. xscore stores the **handles** (`@0`, `%3`) as the
  identity anchors; the numeric index is display data only.
- `PaneInfo.cols/rows/x/y` and `WindowInfo.cols/rows` are copies of what
  tmux reported. `WindowInfo.layout` is parsed from tmux's layout string
  (`even-horizontal` → `split_h`, `even-vertical` → `split_v`, `tiled` →
  `grid`, otherwise `single`).
- `link sync --id <lid>` compares the registry against tmux and reports
  drift; `--heal` corrects registry rows (geometry, dropped rows) from
  tmux. Panes that tmux has but xscore never adopted are reported, never
  adopted behind your back.
- A tmux pane cannot be detached (`TmuxPaneNotDetachable`) — it belongs
  to tmux. `pane del` kills the real tmux pane; `link del` kills the
  session and every pane/window adopted from it.

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
xscore link add <url?> [-c CONFIG] [--id <uuid>] [--pane <pid>]   # link + window + pane
xscore link del --id <link-id>                       # full teardown (tmux: kills session)
xscore link open <url?> [-c CONFIG]                  # attach existing tmux session (tmux only)
xscore link close --id <link-id>                     # tmux-detach only (session keeps running)
xscore link show --id <link-id>
xscore link list
xscore link sync --id <link-id> [--heal]             # diff registry vs tmux (@N / %N)

xscore link config add <name> <url>                   # save a ConnectSpec template
xscore link config remove <name>
xscore link config show <name>
xscore link config list

xscore window add [--name N] [--layout <kind>] [--cols C --rows R] [--link <lid>]
xscore window del --id <wid>
xscore window list                                   # TMUX column shows @N or "-"
xscore window show --id <wid>                        # includes tmux_window_handle
xscore window set-layout --id <wid> --layout <kind>
xscore window add-pane --window <wid> --pane <pid>
xscore window remove-pane --window <wid> --pane <pid>

xscore pane add [--name N] [--cols C --rows R] [--x X --y Y] [--window <wid>]
xscore pane del --id <pid>                           # also kills the tmux pane, if any
xscore pane show --id <pid>                          # includes sink_handle
xscore pane list [--window <wid>] [--link <lid>]
xscore pane attach --pane <pid> --link <lid>         # Q1: auto-detach for raw links
xscore pane detach --pane <pid>                      # direct panes only; buffer kept
xscore pane resize --pane <pid> --cols C --rows R
xscore pane move --pane <pid> --x X --y Y             # geometry only (no bridge)
xscore pane add-to-window --pane <pid> --window <wid>
xscore pane remove-from-window --pane <pid> --window <wid>

xscore data read --pane <pid> [--bytes N]            # errors with NotAttached for orphans
xscore data write --pane <pid> [< file | --from-file F]
xscore data resize --pane <pid> --cols C --rows R

xscore config show                                   # stub for v0.4.0
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
# Default: auto-generated uuid v7 (plus the window and pane it creates)
$ xscore link add shell:///bin/cat
link:   01928e0a-7d10-7000-8000-000000000001
window: 01928e0a-7d10-7000-8000-000000000002
pane:   01928e0a-7d10-7000-8000-000000000003

# Specify a known id
$ xscore link add shell:///bin/cat --id 11111111-2222-7333-8444-555555555555
link:   11111111-2222-7333-8444-555555555555
window: 01928e0a-7d10-7000-8000-000000000004
pane:   01928e0a-7d10-7000-8000-000000000005

# Duplicate id → error
$ xscore link add shell:///bin/echo --id 11111111-2222-7333-8444-555555555555
Error: link id already in use: 11111111-2222-7333-8444-555555555555
```

## Architecture

```
CLI -> SessionManager
       +-- LinkRegistry     (link_id → Arc<Link>)
       +-- WindowRegistry   (window_id → Arc<Window>)         — virtual or tmux @N
       +-- PaneRegistry     (pane_id → PaneInfo)              — virtual or tmux %N
       +-- pane_bridges     (pane_id → Arc<dyn PaneBridge>)   — built on attach/adopt
                                  ▲ wraps:
                                   ├─ DirectBridge { Arc<dyn Transport>, RingBuffer }  (raw)
                                   └─ TmuxBridge { Weak<TmuxSink>, "%N" }              (tmux)
       +-- pane_buffers     (pane_id → Arc<Mutex<RingBuffer>>) — survives detach
```

Each Link owns one Transport (raw IO) + one Sink (tmux orchestration or no-op). Each Pane has its own bridge (the runtime I/O channel) and its own geometry (cols/rows/x/y). Each Window owns its pane list (UI container).

**Lifecycle:**
- `link add` creates the link AND its first window/pane (ADR 0008): a virtual window + buffered pane for raw links, or tmux's own `@N`/`%N` for tmux links.
- `link del` on a **direct** link: every attached pane's `link_id` is cleared and its bridge released, but the transport and the pane buffers stay alive until the link itself is removed. Panes survive as orphans.
- `link del` on a **tmux** link: `kill-session` takes every adopted window/pane with it; they are removed from the registry too.
- `window del` cascades: every pane in the window has its `window_id` cleared. Panes survive as orphans.
- `pane del` fully tears down: also detaches from link + removes from window, and kills the tmux pane if it has a `%N`.

**`add` vs `del` vs `open` vs `close`** (ADR 0005, four-verb `link` tree):
- `add` creates a brand-new external resource: transport + sink, plus its first window/pane.
- `del` is the universal full teardown: cascades pane handling + kills sink + transport. For tmux: kills the per-link session; the shared `-L xscore` server stays up.
- `open` attaches to an existing tmux session (tmux only — shell/serial/ssh return `OpenOnlyForTmux`).
- `close` is a no-op for v0.4.0 (tmux sessions are inherently persistent across xscore invocations).

## Status (v0.4.0)

**Working end-to-end:**
- 3-entity decoupled model with dynamic associations (ADR 0007 v2)
- `link add` auto-creates a usable window + pane (ADR 0008), three ids out
- tmux adoption: `@N` windows / `%N` panes, geometry read from tmux, never guessed
- `link sync [--heal]`: registry ↔ tmux drift report and repair
- `link add --pane`, `window add --link`, `pane add --window`
- Pane buffer: `DirectBridge` + 64 KiB `RingBuffer`, snapshot reads, survives detach
- Q1: direct-link 1-active-pane auto-detach (user-confirmed 2026-10-19)
- Q1 corollary: tmux-backed multi-pane (each `pane add --window` = tmux split)
- Q2: pane geometry (cols/rows/x/y) (user-confirmed 2026-10-19)
- Bridge model: `DirectBridge` (raw) + `TmuxBridge` (tmux) via `PaneBridge` trait
- Local tmux sink: `new-session / new-window / split-window / kill-pane / kill-window / capture-pane / send-keys / resize-pane` over the shared `-L xscore` server (ADR 0003)
- `shell://` transport (local child, real `tokio::process::Command`)
- URL parser: `shell://` / `ssh://` / `serial://` with `?key=value` query
- Persistent ConnectSpec templates: `xscore link config {add,remove,show,list}` at `~/.config/xscore/links/`
- Per-pane data ops: `xscore data {read, write, resize}` via bridge
- 4-verb trees for `link` / `window` / `pane` (ADR 0005)
- State persistence across CLI invocations (`~/.config/xscore/state/state.json`, **schema_version 2** — older files are rejected with an actionable error)
- **Default sink = `raw` (no tmux)**; opt in to tmux with `?tmux=on` or `?tmux=auto|local|remote` in the URL

**Implemented but not exercised by the test suite:**
- `serial://` transport (real `serialport = "4.10"` impl) — needs real hardware
- `ssh://` transport (real `russh = "0.43"` impl) — needs a real SSH server; key auth only (no `known_hosts`, no password)

**Stubbed / pending (deferred to v0.5.0+):**
- `link open` for the local-tmux case is a `todo!()` stub
- `xscore config {show, path, set, unset}` for AppConfig
- `link config edit` for in-place template editing
- Pre-built binaries (currently install via `cargo install --path .`)

**Known limitations (deferred):**
- Direct panes cannot keep live I/O across a process restart (the transport died with the process). Their `PaneInfo` survives; use `pane attach` to resume. tmux panes DO rebuild their bridge on hydration from `%N`.
- `link sync` never adopts panes it did not create — that is deliberate (ADR 0008). Create them through `pane add --window`.
- `xscore link open` for tmux is `todo!()` — will land in v0.5.0.
- SSH `check_server_key` accepts any host key. Real users want `StrictHostKeyChecking=yes` with `~/.ssh/known_hosts`.
- SSH auth is key-only (no password field in `ConnectSpec::Ssh` yet).
- Serial `default-features = false` (no `libudev`); open by explicit path.

## Tests

```bash
# Unit tests (29 tests, in-process, <1s)
cargo test -p xscore --lib

# CLI integration tests (23 tests, ~2s)
cargo test -p xscore --test cli_smoke

# Data-path integration tests (10 tests, single-process, real tmux round-trip)
cargo test -p xscore --test data_smoke

# Everything (62 tests total; --test-threads=1 required — data_smoke sets env vars)
cargo test -p xscore -- --test-threads=1
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
    manager.rs                   # SessionManager (add_link branches, sync_link)
    state.rs                     # save / load (schema_version 2 gate)
  backends/                     # PaneBridge trait + DirectBridge + TmuxBridge
    bridge.rs
    ring_buffer.rs               # per-pane buffer (64 KiB, snapshot reads)
  model/                        # ConnectSpec, TransportConfig, SinkConfig,
                                #   LinkInfo, WindowInfo, WindowLayout, PaneInfo
    session.rs
  transport/                    # Transport trait + 3 impls + factory
    mod.rs
    factory.rs
    shell.rs, serial.rs, ssh.rs
  sink/                         # Sink trait (tmux adoption) + 2 impls + factory
    mod.rs                       # TmuxWindowSnapshot / TmuxPaneSnapshot / layout parsing
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
  cli_smoke.rs                  # 23 CLI integration tests
  data_smoke.rs                 # 10 in-process data-path tests
```

## Release process

See [`RELEASE.md`](RELEASE.md) for the v0.1.0 release checklist and version-bump policy.

## License

MIT (see `LICENSE`).