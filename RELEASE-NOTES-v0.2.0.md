# xscore v0.2.0 — release notes

**Tag:** `v0.2.0`
**Commit:** `efccc6b` (or the final commit before tagging — see `git log`)
**Date:** 2026-10-19
**Test count:** 37 (20 unit + 14 cli_smoke + 3 data_smoke), 0 failed, 0 ignored
**Warnings:** 0

> **Breaking behavior change vs v0.1.0:** the default sink is now
> `raw` (direct transport I/O, no tmux server). The full Link × Window ×
> Pane tree is opt-in via `?tmux=on` in the URL. See
> [ADR 0007](doc/dev/adr/0007-raw-default-tmux-opt-in.md) for the
> rationale. **No state-file migration needed** — the `SinkConfig` enum
> gained a `Raw` variant alongside the existing `Tmux` variant; old
> `state.json` files load unchanged.

## What's new in v0.2.0

### Default sink = `raw`; tmux is opt-in

- `xscore link add <url>` (no `?tmux=` query): the link uses a
  `RawSink`. The transport is the I/O source directly. No tmux server
  in the loop. Only `link: <id>` is printed.
- `xscore link add 'shell:///bin/bash?tmux=on'`: the link uses a
  `TmuxSink` (mode=auto). One tmux session is created in the shared
  `tmux -L xscore` server, with one default window + one default pane
  auto-created. (This is the v0.1.0 behavior, now opt-in.)
- `xscore window add` / `xscore pane add` / `xscore data {read,write,
  resize}` return `Error::NotTmuxLink(<id>)` for raw links. Add
  `?tmux=on` to the URL to use them.

### URL `?tmux=` query options

| `?tmux=` value | Sink | Window/Pane tree | Data ops |
|---|---|---|---|
| _absent_ (default) | `raw` | no | (no panes) |
| `?tmux=off` | `raw` | no | (no panes) |
| `?tmux=on` | `tmux` (mode=auto) | yes | yes |
| `?tmux=auto` | `tmux` (mode=auto) | yes | yes |
| `?tmux=local` | `tmux` (mode=local) | yes | yes |
| `?tmux=remote` | `tmux` (mode=remote) | yes | yes |

`auto` resolves at link-creation time: `ssh://` → `remote` (tmux on
the remote host), `shell://` / `serial://` → `local` (the local shared
`tmux -L xscore` server). `name=X` overrides the auto-generated
`xscore-<link_id>` tmux session name; only meaningful with `?tmux=...`.

### Other v0.1.0 → v0.2.0 changes

- `6aa14c0` — `fix(sink)`: close tmux server-internal race in
  `TmuxSink::new_pane` (50ms sleep + `list-panes` after `split-window`).
  Workaround for tmux's async race where `split-window` returns success
  before the server commits state. Affects `tmux=on` users only.
- `579f3e9` + `abbdd6f` — column headers on all `list` commands
  (`link`, `window`, `pane`, `link config`).

## Quick smoke (5 minutes)

```bash
# New default: raw link (no tmux)
$ xscore link add shell:///bin/bash
link:  01928e0a-7d10-7000-8000-000000000001

# State is persisted to ~/.config/xscore/state/state.json
$ xscore link list
LINK ID                              NAME
01928e0a-...                        01928e0a-...

# Opt in to tmux for window/pane tree + data ops
$ xscore link add 'shell:///bin/bash?tmux=on'
link:   01928e0a-7d10-7000-8000-000000000002
window: 01928e0a-7d10-7000-8000-000000000003
pane:   01928e0a-7d10-7000-8000-000000000004

# Add window + pane, then read/write (uses tmux capture-pane / send-keys)
$ xscore window add --link <link-id-2> --name dev
$ xscore pane add   --window <window-id> --name bash
$ echo -n 'echo hi' | xscore data write --pane <pane-id>
$ xscore data read --pane <pane-id>

# Clean up
$ xscore link del --id <link-id-1>
$ xscore link del --id <link-id-2>
```

Full 5-minute demo with all 9 steps + verified output: see
[`README.md`](README.md#5-minute-demo).

## Install

```bash
cargo install --path .       # from the workspace root
# or
git clone <repo> xscore && cd xscore
git checkout v0.2.0
cargo build --release
./target/release/xscore --help
```

**Requirements:** Rust 1.95+. tmux 3.0+ is **only required** if you opt
in to the tmux sink via `?tmux=...` in the URL.

## What's working

- **1-tier raw links** (new default): `xscore link add <url>` creates
  a `RawSink` link. The transport is the I/O source directly. No
  tmux server, no install requirement, no race bugs.
- **3-tier tmux-backed links** (`?tmux=on`): Link × Window × Pane
  (ADR 0006) wired end-to-end, with one default window + pane
  auto-created.
- **Local tmux sink** via the shared `tmux -L xscore` server (ADR 0003):
  `new-session` / `new-window` / `split-window` / `kill-*` /
  `capture-pane` / `send-keys` / `resize-pane`.
- **URL parser**: `shell://` / `serial://` / `ssh://` with
  `?key=value` query.
- **`shell://` transport** (real `tokio::process::Command` — child
  process with piped stdin/stdout).
- **`serial://` transport** (real `serialport = "4.10"` impl — opens
  the device with `baud`/`data_bits`/`parity`/`stop_bits`).
- **`ssh://` transport** (real `russh = "0.43"` impl — key-based
  auth, session channel with `request_shell`).
- **Persistent ConnectSpec templates** at `~/.config/xscore/links/`
  (`xscore link config {add,remove,show,list}`).
- **Per-pane data ops** (`xscore data {read, write, resize}` over
  tmux) for tmux-backed links.
- **4-verb trees** for `link` / `window` / `pane` (ADR 0005 + ADR 0006).
- **State persistence** across CLI invocations
  (`~/.config/xscore/state/state.json`, atomic write + rename).
- **ADR 0005 invariants**: `link open` and `link close` reject
  non-tmux transports with `OpenOnlyForTmux` / `CloseOnlyForTmux`.

## Known limitations (deferred to v0.3.0+)

These are documented in [`CHANGELOG.md`](CHANGELOG.md) under
"Unreleased":

- `xscore link open` for the local-tmux case is a `todo!()` stub
  (manager.rs `open_link`); non-tmux correctly returns `OpenOnlyForTmux`
- SSH `check_server_key` accepts any host key — no `~/.ssh/known_hosts`
  lookup
- SSH auth is key-only — no `?password=` query param yet
- Serial `default-features = false` (no `libudev`) — open by explicit
  path only
- Hydration uses `NoopTransport` for I/O — `data read`/`data write` on
  a link that survives a process restart fail until the user re-adds
  the link (the original child process / SSH channel is gone)
- `xscore config {show, path, set, unset}` (top-level config —
  separate from `xscore link config` templates)
- Pre-built binaries (currently `cargo install --path .`)

## Architecture

```
CLI → SessionManager
       ├── LinkRegistry     (by_id, by_address)         — one Link per physical connection
       ├── WindowRegistry   (by_id, by_link)           — one Window per tmux window (ADR 0006; tmux links only)
       ├── PaneRegistry     (pane_id → PaneInfo)       — one Pane per tmux split-window (tmux links only)
       └── BackendRegistry  (pane_id → Arc<dyn PaneBackend>)
                                                ^ wraps (Link, sink_handle)
```

Each Link owns one `Transport` (raw IO) + one `Sink` (`RawSink` for
raw links, `TmuxSink` for tmux-backed links). Tmux-backed links own N
windows in the shared `tmux -L xscore` server; each window owns M
panes (tmux split-windows). Raw links have no windows or panes.
See [`README.md`](README.md#architecture) for the full architecture
doc and [`doc/dev/adr/0007-raw-default-tmux-opt-in.md`](doc/dev/adr/0007-raw-default-tmux-opt-in.md)
for the design pivot.

## Tests (37 total)

| Area | Where | Count |
|---|---|---|
| URL parsing (incl. sink opt-in) | `url::connect::tests` | 11 |
| Core id roundtrips + distinctness | `core::id::tests` | 3 |
| `LinkAddress` equality | `core::address::tests` | 2 |
| `Transport` trait object-safety | `transport::tests` | 1 |
| `ShellTransport` cat round-trip | `transport::shell::tests` | 1 |
| `Sink` trait object-safety | `sink::tests` (noop) | 1 |
| CLI help / validation / invariants | `tests/cli_smoke.rs` | 14 |
| In-process data path + double-window fix + cross-manager hydration | `tests/data_smoke.rs` | 3 |

```bash
cargo test -p xscore                    # all 37
cargo test -p xscore --lib              # 20 unit only
cargo test -p xscore --test cli_smoke   # 14 CLI
cargo test -p xscore --test data_smoke  # 3 data path
```

## Migration from v0.1.0

1. **State files:** no migration needed. `SinkConfig::Tmux` variant
   is unchanged; existing v0.1.0 state.json files load fine.
2. **CLI scripts:** add `?tmux=auto` to URLs that need the
   window/pane tree. Find+replace: `shell:///bin/bash` →
   `shell:///bin/bash?tmux=auto` (or wherever a tmux-backed link
   was previously expected).
3. **Tests:** any `xscore link add` test that expects `window:` /
   `pane:` lines must add `?tmux=...` to the URL. (Updated in
   commit `efccc6b` — 4 cli_smoke + 3 data_smoke URLs.)
4. **API users:** `add_link` / `open_link` now return
   `Option<WindowId>` / `Option<PaneId>`. Handle the None case for
   raw links (no auto-created window/pane).

## Links

- **Source / docs:** see `doc/dev/` for the full design history
  (7 ADRs: 0001, 0002, 0003, 0005, 0006, 0007 + archived 0004).
- **CHANGELOG:** [`CHANGELOG.md`](CHANGELOG.md)
- **Release process:** [`RELEASE.md`](RELEASE.md)
- **Architecture overview:** [`doc/dev/roadmap/overview.md`](doc/dev/roadmap/overview.md)

## Credits

Built with: `tokio`, `clap`, `serde`, `russh`, `serialport`,
`async-trait`, `uuid`, `tracing`, `portable-pty`, `thiserror`,
`anyhow`, `dirs`, `chrono`. Cargo workspace; one binary crate
`crates/xscore`. License TBD (placeholder; pick at v1.0).
