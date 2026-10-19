# xscore v0.3.0 — release notes

**Tag:** `v0.3.0`
**Date:** 2026-10-20
**Test count:** 38 (20 unit + 13 cli_smoke + 5 data_smoke), 0 failed, 0 ignored
**Warnings:** 0

> **⚠️ BREAKING change vs v0.2.0:** the data model is redesigned.
> Three entities (Link, Pane, Window) are now independent — no more
> `Link → Window → Pane` hierarchy. The state schema is rewritten
> (flat `{links, windows, panes}` instead of nested `Vec<LinkSnapshot>`).
> See [ADR 0007](doc/dev/adr/0007-raw-default-tmux-opt-in.md) for the
> rationale. **No auto-migration** — users must `rm ~/.config/xscore/state/state.json`
> on upgrade and re-add links.

## What's new in v0.3.0

### 3-entity decoupled model

The 3-tier `Link → Window → Pane` hierarchy from v0.2.0 is replaced by
three independent entities:

- **Link** = transport + sink + active-pane-cache (Q1 tracking).
- **Pane** = buffer (bridge) + UI info (cols/rows/x/y + which link + which window).
- **Window** = pure UI layout container (layout + pane list + total cols/rows).

Each is orphan-creatable:

- `xscore link add <url>` — creates ONLY the link.
- `xscore window add` — creates ONLY a window (no link required).
- `xscore pane add` — creates ONLY an orphan pane (no link, no window).

Associations are wired by explicit verbs:

- `xscore pane attach --pane <pid> --link <lid>` — bind pane to link (creates the bridge).
- `xscore pane detach --pane <pid>` — unbind (buffer preserved).
- `xscore pane add-to-window --pane <pid> --window <wid>` — wire pane into a window.
- `xscore pane remove-from-window --pane <pid> --window <wid>` — remove from window.

### Q1: direct link 1-active-pane rule (user-confirmed 2026-10-19)

For shell/ssh/serial links (raw), only one pane may be active at a time.
When a new pane is attached via `pane attach`, the previously-active
pane is auto-detached (its `link_id` becomes `None`, its buffer is
preserved for re-attach later).

For tmux-backed links (`?tmux=on|auto|local|remote`), multiple panes
may attach simultaneously — each `attach_pane` creates a new tmux
split-window in the link's session. No auto-detach.

### Q2: pane geometry (user-confirmed 2026-10-19)

A pane's UI info is geometry — `cols`, `rows`, `x`, `y`. Each pane
declares its own size and position independently of its parent window.

### Bridge model

The new `backends::bridge` module exposes the per-pane runtime:

- `PaneBridge` trait — `kind()`, `read()`, `write()`, `resize()`, `close()`.
- `DirectBridge` — wraps the link's `Arc<dyn Transport>` for raw links.
- `TmuxBridge` — wraps `Weak<TmuxSink>` for tmux-backed links.
  - `read` → `tmux capture-pane` (scrollback).
  - `write` → `tmux send-keys`.
  - `resize` → `tmux resize-pane`.
  - `close` → `tmux kill-pane`.

Bridges are stored SEPARATELY from `PaneInfo` (in
`SessionManager.pane_bridges`). This way mutating `PaneInfo` (which is
JSON-serializable) doesn't destroy the bridge.

### State schema

The state file (`~/.config/xscore/state/state.json`) now contains:

```json
{
  "links":   [LinkInfo, ...],
  "windows": [{...WindowInfo..., "panes": ["<pid>", ...]}, ...],
  "panes":   [PaneInfo, ...]
}
```

`PaneInfo.link_id` and `PaneInfo.window_id` carry the associations.
The v0.2.0 nested `Vec<LinkSnapshot>` is GONE. Bridges are not persisted
(they're rebuilt on `pane attach`).

**Migration from v0.2.0:** delete `~/.config/xscore/state/state.json`
on upgrade; re-add links. There is no auto-migration.

### URL `?tmux=` query options (unchanged from v0.2.0)

| `?tmux=` value | Sink | Window/Pane tree | Data ops | Q1 (1 active pane) |
|---|---|---|---|---|
| _absent_ (default) | `raw` | optional, user creates | via `pane attach` | yes (auto-detach) |
| `?tmux=on` | `tmux` (mode=auto) | optional, user creates | via `pane attach` | no (multi-pane OK) |
| `?tmux=auto` | `tmux` (mode=auto) | optional | via `pane attach` | no |
| `?tmux=local` | `tmux` (mode=local) | optional | via `pane attach` | no |
| `?tmux=remote` | `tmux` (mode=remote) | optional | via `pane attach` | no |

## Quick smoke (5 min)

```bash
$ cargo build --release

# 1. Add a link (default: raw, no auto window/pane)
$ ./target/release/xscore link add shell:///bin/bash
link:  01928e0a-...

# 2. Add an orphan pane
$ ./target/release/xscore pane add
pane:  01928e0a-...

# 3. Attach the pane to the link — creates the bridge
$ ./target/release/xscore pane attach --pane 01928e0a-... --link 01928e0a-...
attached: 01928e0a-...

# 4. Data I/O via the bridge
$ ./target/release/xscore data write --pane 01928e0a-... <<< 'echo hi'
$ ./target/release/xscore data read --pane 01928e0a-...

# 5. Create a window + put the pane in it
$ ./target/release/xscore window add --cols 120 --rows 30
window: 01928e0a-...
$ ./target/release/xscore pane add-to-window --pane 01928e0a-... --window 01928e0a-...

# 6. Verify state
$ ./target/release/xscore pane show --id 01928e0a-...
# shows link_id, window_id, geometry (cols/rows/x/y)
$ ./target/release/xscore window list
$ ./target/release/xscore pane list --window 01928e0a-...

# 7. Detach the pane (buffer preserved)
$ ./target/release/xscore pane detach --pane 01928e0a-...

# 8. Re-attach to the same or a different link
$ ./target/release/xscore pane attach --pane 01928e0a-... --link 01928e0a-...

# 9. Cleanup
$ ./target/release/xscore link del --id 01928e0a-...
$ ./target/release/xscore window del --id 01928e0a-...
$ ./target/release/xscore pane del --id 01928e0a-...
```

## Install

```bash
# From the workspace root:
cargo install --path .

# Or clone and run from source:
git clone <repo> xscore && cd xscore
git checkout v0.3.0
cargo build --release
./target/release/xscore --help
```

**Requirements:**
- Rust 1.95+
- `tmux` 3.0+ on `$PATH` (only for `?tmux=on|auto|local|remote` links)
- A real serial device or SSH server for the non-shell transports (optional)

## What's working

- 3-entity decoupled model with dynamic associations (ADR 0007 v2).
- Q1: direct-link 1-active-pane auto-detach (user-confirmed 2026-10-19).
- Q1 corollary: tmux-backed multi-pane.
- Q2: pane geometry (cols/rows/x/y) — user-confirmed 2026-10-19.
- `PaneBridge` trait + `DirectBridge` (raw) + `TmuxBridge` (tmux).
- New CLI verbs: `pane {add, attach, detach, resize, move, add-to-window,
  remove-from-window, show, list}` and `window {add, set-layout, add-pane,
  remove-pane, ...}`.
- Flat state schema with `PaneInfo.link_id` / `window_id`.
- Local tmux sink via the shared `tmux -L xscore` server (ADR 0003).
- `shell://` transport (local child via `tokio::process::Command`).
- URL parser: `shell://` / `ssh://` / `serial://` with `?key=value` query.
- Persistent ConnectSpec templates: `xscore link config {add,remove,show,list}`.
- Per-pane data ops: `xscore data {read, write, resize}` (via bridge).
- 4-verb `link` tree (`add` / `del` / `open` / `close`) — `open`/`close`
  remain tmux-only (ADR 0005 invariant).
- State persistence across CLI invocations
  (`~/.config/xscore/state/state.json`, **new flat schema**).
- Default sink = `raw` (no tmux); opt in via `?tmux=on|auto|local|remote`.

## Architecture diagram (v0.3.0)

```
SessionManager
├── LinkRegistry     (link_id → Arc<Link>)
├── WindowRegistry   (window_id → Arc<Window>)         — independent, no link
├── PaneRegistry     (pane_id → PaneInfo)              — orphan-creatable
└── pane_bridges     (pane_id → Arc<dyn PaneBridge>)   — built on attach_pane

Link { id, info, transport: Arc<dyn Transport>, sink: Box<dyn Sink>,
       tmux_sink: Option<Arc<TmuxSink>>, active_pane: Mutex<Option<PaneId>> }

PaneInfo { id, name, cols, rows, x, y,                 // Q2 geometry
          link_id: Option<LinkId>,                     // Q1 source-of-truth
          window_id: Option<WindowId> }

WindowInfo { id, name, layout: WindowLayout, cols, rows }   // pure UI

PaneBridge (trait)
├── DirectBridge  { transport: Arc<dyn Transport> }          // raw links
└── TmuxBridge    { sink: Weak<TmuxSink>, handle: String }   // tmux links
```

## Tests (38 total, 0 ignored)

```bash
cargo test -p xscore --lib              # 20 unit tests
cargo test -p xscore --test cli_smoke    # 13 CLI integration tests
cargo test -p xscore --test data_smoke   #  5 in-process data-path tests
cargo test -p xscore                     # all 38
```

| Area | Where | Count |
|---|---|---|
| URL parsing (incl. sink opt-in) | `url::connect::tests` | 11 |
| Core id roundtrips + distinctness | `core::id::tests` | 3 |
| `LinkAddress` equality | `core::address::tests` | 2 |
| `Transport` trait object-safety | `transport::tests` (echo) | 1 |
| `ShellTransport` cat round-trip | `transport::shell::tests` | 1 |
| `Sink` trait object-safety | `sink::tests` (noop) | 1 |
| Sink opt-in &amp; tmux-on variant | `url::connect::tests` (more) | 1 |
| CLI help tree | `cli_smoke` | 1 |
| CLI input validation (link add needs url) | `cli_smoke` | 1 |
| ADR 0005 invariant (link open non-tmux) | `cli_smoke` | 1 |
| Disk-based config templates (link config add/list) | `cli_smoke` | 1 |
| Two `link add <same url>` = two distinct ids | `cli_smoke` | 1 |
| **Default sink = raw; only `link:` printed** | `cli_smoke` | 1 |
| `?tmux=on` creates ONLY a link (no auto window/pane) | `cli_smoke` | 1 |
| `link add --id <uuid>` — explicit id | `cli_smoke` | 1 |
| `link add --id <dup>` — duplicate-id error | `cli_smoke` | 1 |
| **v0.3.0 full flow** (add → add → attach → window → add-to-window → list → detach) | `cli_smoke` | 1 |
| **v0.3.0 Q1 direct-link auto-detach** | `cli_smoke` | 1 |
| v0.3.0 window orphan (add/list/del) | `cli_smoke` | 1 |
| **v0.3.0 link del detaches panes (cascade)** | `cli_smoke` | 1 |
| In-process add → attach → write → snapshot | `data_smoke` | 1 |
| **tmux link supports multiple attached panes (Q1 corollary)** | `data_smoke` | 1 |
| **direct link auto-detaches previously-active pane (Q1)** | `data_smoke` | 1 |
| Cross-manager hydration (manager A → manager B) | `data_smoke` | 1 |
| **orphan pane data ops return NotAttached** | `data_smoke` | 1 |

Total: **38 tests, 0 ignored.**

## Migration from v0.2.0

**No auto-migration.** The state schema is incompatible.

```bash
# Backup your v0.2.0 state (optional — for reference only)
mv ~/.config/xscore/state/state.json ~/.config/xscore/state/state.v0.2.0.bak

# Clear v0.2.0 state and re-add links in v0.3.0
rm ~/.config/xscore/state/state.json
xscore link add shell:///bin/bash           # recreate your links
xscore pane add                             # create orphan panes
xscore pane attach --pane <pid> --link <lid>  # wire up data I/O
xscore data write --pane <pid> <<< 'hello'  # verify data path
```

The CLI prints helpful error messages if you try to read a v0.2.0
state file in v0.3.0 — but it won't crash the program; the error is
returned to the CLI and the manager starts empty.

## Known limitations (deferred to v0.4.0+)

- **Ring buffer for direct bridges** — `DirectBridge.read` is one-shot
  (`transport.read(buf)`); a local ring buffer sidecar (so detached
  panes keep their history) is deferred.
- **`xscore link open` for tmux** — still a `todo!()` stub. The
  non-tmux invariant (`OpenOnlyForTmux`) IS wired and tested.
- **`xscore config {show, path, set, unset}`** for AppConfig (separate
  from `xscore link config` templates).
- **SSH password auth + `known_hosts`** — `SshTransport` accepts any
  host key (no `StrictHostKeyChecking`).
- **Pre-built binaries** — install via `cargo install --path .`.
- **Multi-window tmux link semantics** — v0.3.0 places all panes in
  a tmux link's window index 0. Future versions may let panes pick
  which tmux window to split inside.

## See also

- [ADR 0007](doc/dev/adr/0007-raw-default-tmux-opt-in.md) — the 2026-10-20
  design pivot to the 3-entity decoupled model.
- [doc/dev/roadmap/overview.md](doc/dev/roadmap/overview.md) — architecture
  overview + diagrams.
- [CHANGELOG.md](CHANGELOG.md) — full v0.3.0 changelog entry.

## License

MIT (see `LICENSE`).