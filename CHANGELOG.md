# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.5.0] - 2026-10-20

A resident daemon, so direct links stop dying with the CLI process. See
[ADR 0009](doc/dev/adr/0009-daemon.md).

### Added

- **`xscore daemon serve` / `status` / `stop`** — a resident process that
  owns every direct-link `Transport` (shell child, serial port, ssh
  channel) and every direct pane's `RingBuffer`. This is what makes
  `link add` in one process and `data write` in another work at all.
- **Auto-start**: the first command that needs a transport (`link add`,
  `pane attach`) spawns the daemon detached and polls the socket for up
  to 2 s. Nothing to configure, no systemd unit, no shell daemon.
- **`daemon status`** prints JSON: the daemon's pid, the link ids it
  holds, and per-pane `{pane, link, buffered}`.
- `daemon/protocol.rs` — length-prefixed JSON framing
  (`[u32 LE length][JSON]`), hex payload encoding, a dozen ops
  (`ping`, `status`, `create_link`, `delete_link`, `create_pane`,
  `delete_pane`, `attach`, `detach`, `read`, `write`, `resize`, `stop`).
  The transport payload is `model::TransportConfig`'s own serde shape, so
  there is no second transport vocabulary.
- `backends::DaemonBridge` — a `PaneBridge` whose `read`/`write`/
  `resize`/`close` are socket requests. `DirectBridge` stays for
  in-process use (`data_smoke` exercises it).
- `daemon::DaemonTransport` — the `Transport`-shaped handle a daemon-backed
  `Link` holds; the real transport lives in the daemon.
- `SessionManager::enable_daemon`, `SessionManager::link_is_connected`.
- New error: `Error::Daemon(String)`.
- 25 new unit tests (protocol framing/hex/`op` shapes, the daemon's
  attach/detach/delete/Q1 state machine against a scripted transport) and
  5 new cross-process CLI tests, including
  `xscore_daemon_direct_link_survives_process` — create in process A,
  write in B, read in C.

### Changed

- **Direct links are daemon-backed in the CLI.** `link add` (raw sink)
  now: ensure daemon → `create_link` → `create_pane` → write metadata →
  `attach` → `save_state`. `link del` sends `delete_link` (the daemon
  closes the transport once and unbinds its panes, keeping their
  buffers). `pane attach`/`pane detach`/`pane del` keep the daemon in
  sync — and `attach` doubles as a **reconnect**, re-creating the
  transport from the persisted `TransportConfig` if the daemon lost it.
- **`link show`'s `connected` is now a live probe** of the daemon
  (`{"op":"status"}`) instead of a flag recorded at hydration, so
  `daemon stop` → `connected: false` and a re-attach → `connected: true`.
- **`data read` on a detached direct pane** now serves the daemon-held
  buffer instead of failing: a pane's core is its buffer (ADR 0008), and
  the buffer outlives the binding. `data write`/`resize` still require an
  attached link and still fail with "not attached".
- tmux links are untouched: `TmuxBridge`/`TmuxSink` still drive tmux
  directly, because tmux is already resident.
- The Q1 rule (one active pane per direct link) is now enforced on both
  sides: the manager for metadata, the daemon for pumps.

### Fixed

- **`state.json` temp-file race.** Two concurrent CLI invocations both
  wrote `state.json.tmp`; the loser's `rename` then failed with ENOENT
  because the winner had already consumed the file. The temp name is now
  per-process (`state.json.<pid>.tmp`).
- **Single-instance startup.** Two processes auto-starting the daemon at
  the same moment could each unlink-and-bind the socket, leaving two
  daemons on one path (each holding some of the links, forever). Startup
  now takes a `daemon.lock` with `create_new` before binding, and a lock
  whose socket no longer answers is reclaimed.
- A failed `link add` (attach or state-save error) no longer leaves a
  daemon holding a transport nothing references — and therefore a daemon
  that could never idle-exit.

### Note on v0.4.1

v0.4.1's `xscore_direct_link_reports_not_connected_after_restart` was the
honest description of the old limitation. It is now
`xscore_direct_link_stays_connected_after_restart`, asserting
`connected: true` and a successful cross-process write: the same scenario,
fixed.

## [0.4.1] - 2026-10-20

### Fixed

- **Connection liveness is now modelled separately from the binding.**
  A pane's `link_id` says "which link this pane belongs to"; a link now
  also carries `connected`. A direct link hydrated from disk keeps its
  binding and metadata but reports `connected: false`, because its
  transport (child process / serial fd / ssh channel) died with the
  process that created it. Before this, `pane show` said the pane was
  attached while `data read` said it was not — persisted state and
  runtime state contradicted each other.
  - `link show` now includes a `connected` field.
  - Data ops on a bound-but-dead link return the new
    `Error::LinkNotConnected(link_id)` ("…not connected (its transport
    died with the previous process); re-add the link to reconnect")
    instead of the misleading `NotAttached`, which wrongly implies the
    pane was never bound. Orphan panes still get `NotAttached`.
  - tmux links stay `connected: true` after hydration: the tmux session
    outlives the CLI process and `%N` handles rebuild the bridge.
  - NOT fixed by this release: direct links still cannot keep live I/O
    across a process boundary. Lifting that needs a long-lived daemon
    owning the transport (tracked for a future release).

- **Serial read timeout no longer starves writes.** `SerialTransport`
  opened the port with a 200 ms read timeout while the direct-bridge
  reader pump polls with a 10 ms timeout. Because the transport is
  synchronous, each pump tick aborted its await and left a blocking
  task queued behind the port mutex — writes stalled and blocking
  threads accumulated. The read timeout is now 5 ms (< the pump's
  10 ms poll), so each blocking read completes inside one pump tick.
  On Linux this timeout governs reads only (VMIN/VTIME), so writes are
  unaffected.

## [0.4.0] - 2026-10-20

tmux adoption, real pane buffers, and `link add` that hands you a usable
pane. See [ADR 0008](doc/dev/adr/0008-tmux-adoption.md).

### BREAKING — `link add` now creates a window and a pane

`xscore link add <url>` returns three ids: link, window, pane. The link
is immediately usable (`data write/read` work without any further
setup). For `?tmux=on` links, the window/pane are **adopted** from the
tmux session rather than guessed.

- `xscore link add <url>` → virtual window (120×30) + pane (80×24) with
  a ring buffer.
- `xscore link add '<url>?tmux=on'` → the session's window (`@N`) and
  pane (`%N`), with tmux's geometry.
- `xscore link add <url> --pane <pid>` → bind an existing pane; no
  window is created. Errors with `PaneAlreadyAttached` if that pane is
  already attached (never steals), and with `TmuxLinkCannotBindPane`
  for tmux links (tmux creates its own panes).

### BREAKING — state schema v2

`state.json` now carries `"schema_version": 2`. Loading anything else is
a hard error:

```
state.json schema_version=0, expected 2; delete ~/.config/xscore/state/state.json to reset
```

Delete `~/.config/xscore/state/state.json` when upgrading from v0.3.0.
There is no migration: the v0.3.0 rows lack the tmux identity fields.

### Added

- `link sync --id <lid> [--heal]` — diff xscore's registry against
  tmux's real windows/panes (`@N`/`%N`), and optionally heal registry
  geometry/rows. Panes created outside xscore are reported, never
  adopted.
- `link add --pane <pid>` — bind an existing pane to a new direct link.
- `window add --link <lid>` — create a real tmux window (adopting `@N`).
- `pane add --window <wid>` — create a tmux split inside that window,
  with tmux's geometry.
- `PaneInfo.sink_handle: Option<String>` (`%N`) and
  `WindowInfo.tmux_window_handle: Option<String>` (`@N`) — the identity
  anchors that make restart-time reconciliation possible.
- `Sink::{list_windows, list_panes, new_window, kill_window,
  split_pane}` and `TmuxWindowSnapshot`/`TmuxPaneSnapshot` — structure
  is read back from tmux, never predicted.
- `backends/ring_buffer.rs` — `RingBuffer` (64 KiB) with non-destructive
  snapshot reads.
- `link sync` for tmux links; tmux bridges are rebuilt on hydration from
  the stored `%N` handle.
- New errors: `TmuxLinkCannotBindPane`, `PaneAlreadyAttached`,
  `TmuxPaneNotDetachable`.
- Unit tests for the ring buffer, the schema gate and tmux layout
  parsing; CLI/data tests for all of the above (62 tests total).

### Changed

- `detach_pane` on a tmux pane is now an error (`TmuxPaneNotDetachable`);
  use `pane del` (which kills the real tmux pane).
- `link del` on a tmux link removes the adopted windows and panes from
  the registry along with the session; on a direct link it orphans the
  panes and keeps their buffers, closing the transport exactly once.
- `TmuxSink::split_pane` takes an explicit window handle — there is no
  implicit window 0.
- `Window.layout` for tmux windows is derived from tmux's layout string;
  `--layout/--cols/--rows` are ignored when `--link` is given (tmux
  wins).
- `pane add --window` requires the window to exist; tmux panes always
  get a `link_id` from the link that owns the window.

### Fixed

- **P0-a:** `detach_pane` no longer closes the link's transport.
  `DirectBridge::close()` now stops only its own reader task, so
  detach → re-attach works and other panes keep their I/O.
- **P0-b:** `DirectBridge` has a real buffer. A background task drains
  the transport into a per-pane `RingBuffer`; `data read` serves a
  snapshot of it (tmux `capture-pane` semantics) instead of a one-shot
  pass-through. The buffer survives detach and re-attach.
- **P0-c / P0-d:** `PaneInfo.sink_handle` / `WindowInfo.tmux_window_handle`
  record tmux identity, so `PaneId`/`WindowId` map back to `%N`/`@N`
  across restarts.
- **P0-e:** `state.json` without a matching `schema_version` is an
  explicit error instead of a silently empty registry.
- The reader task polls `transport.read()` under a short timeout so a
  read-blocking transport (which serializes read/write behind one lock,
  as `ShellTransport` does) cannot starve `data write`.

## [0.3.0] - 2026-10-20

### BREAKING — 3-entity decoupled model (ADR 0007 v2)

The whole data model is redesigned. Three independent entities
(Link, Pane, Window) replace the previous 3-tier
`Link → Window → Pane` hierarchy. Each entity is orphan-creatable
(no link required); associations are dynamic Option IDs on `PaneInfo`.

- `xscore link add <url>` now creates ONLY the link. NO auto
  window, NO auto pane. (v0.2.0 created one default window + one
  default pane for tmux-backed links.)
- New verbs (v0.3.0):
  - `xscore pane add [--cols C --rows R --x X --y Y]`
  - `xscore pane attach --pane <pid> --link <lid>` — bind pane to link (creates bridge)
  - `xscore pane detach --pane <pid>` — unbind (buffer preserved)
  - `xscore pane resize --pane <pid> --cols C --rows R`
  - `xscore pane move --pane <pid> --x X --y Y` — update geometry only
  - `xscore pane add-to-window --pane <pid> --window <wid>`
  - `xscore pane remove-from-window --pane <pid> --window <wid>`
  - `xscore window add [--layout <kind>] [--cols C --rows R]` — orphan
  - `xscore window set-layout --id <wid> --layout <kind>`
  - `xscore window add-pane --window <wid> --pane <pid>`
  - `xscore window remove-pane --window <wid> --pane <pid>`
- Removed verbs / options:
  - `xscore pane add --window <wid>` (v0.2.0) → `xscore pane add` (v0.3.0, orphan)
  - `xscore window add --link <lid>` (v0.2.0) → `xscore window add` (v0.3.0, orphan)
  - `xscore window list --link <lid>` filter — windows are not link-scoped anymore
  - `xscore pane list --window <wid>` filter kept; `--link <lid>` filter added

### BREAKING — state schema flat

The state file (`~/.config/xscore/state/state.json`) is rewritten
from a nested `Vec<LinkSnapshot>` to a flat `{links, windows, panes}`
object. **No migration path** — users upgrading from v0.2.0 must
`rm ~/.config/xscore/state/state.json` on upgrade and re-add their
links. Documented in the release notes.

### Added

- `PaneInfo.geometry` (cols/rows/x/y) — pane's UI info per Q2
  (user-confirmed 2026-10-19).
- `PaneInfo.link_id: Option<LinkId>` and `PaneInfo.window_id: Option<WindowId>`
  — explicit association pointers (vs. v0.2.0's nested hierarchy).
- `WindowInfo.layout: WindowLayout` — `single` | `tabs` | `split_h`
  | `split_v` | `grid{cols,rows}`. Pure UI hint, independent of any
  link.
- `WindowInfo.cols`/`rows` — total window size (vs. pane's individual
  geometry).
- `Link.transport: Arc<dyn Transport>` (was `Box<dyn Transport>`) —
  needed so `DirectBridge { transport: Arc<dyn Transport> }` can share.
- `Link.tmux_sink: Option<Arc<TmuxSink>>` (NEW) — concrete Arc for
  `TmuxBridge { sink: Weak<TmuxSink> }`.
- `Link.active_pane: Mutex<Option<PaneId>>` (NEW) — Q1 cache for
  direct links (NOT the source of truth — `PaneInfo.link_id` is).
- `Error::NotAttached(PaneId)` — returned by data ops on orphan panes.
- `Error::AlreadyInWindow { window, pane }` and
  `Error::NotInWindow { window, pane }` — explicit add/remove errors.
- `Error::PaneNotOnLink { link, pane }` — for symmetric detach checks.
- New `backends::bridge` module — `PaneBridge` trait + `DirectBridge`
  + `TmuxBridge` impls. The legacy `PaneBackend` trait,
  `BackendRegistry`, and `CompositeBackend` are REMOVED.
- `SessionManager.pane_bridges: Arc<Mutex<HashMap<PaneId, Arc<dyn PaneBridge>>>>`
  — separate from `PaneRegistry` so mutating `PaneInfo` doesn't
  destroy the bridge.

### Changed

- `xscore window add` no longer takes `--link`. Windows are independent.
- `xscore pane add` no longer takes `--window`. Panes are orphan.
- `xscore link del` cascades: every attached pane's `link_id` is
  cleared, bridge closed silently. The panes themselves survive as
  orphans.
- `xscore window del` cascades: every pane in the window has its
  `window_id` cleared. Panes survive as orphans.
- Hydration no longer auto-builds bridges. The user must
  `xscore pane attach` after restart to resume data I/O.
- The Sink trait lost `new_window` and `kill_window` (windows are
  independent now). `new_pane` no longer takes a window index — all
  tmux panes go into the link's single tmux window (window_idx 0).

### Implementation detail

- v0.3.0 implements the user-confirmed Q1 rule: **direct (raw) links
  may have only ONE active pane at a time.** When a different pane
  is attached, the previously-active pane is auto-detached (its
  `link_id` becomes `None`, its buffer is preserved). For tmux-backed
  links, multiple panes may attach simultaneously (Q1 corollary).
- v0.3.0 implements the user-confirmed Q2 rule: **pane's UI info =
  geometry** — `cols`, `rows`, `x`, `y`. Each pane declares its own
  size and position independently of its parent window.

## [0.2.0] - 2026-10-19

### Changed

- **Design pivot: tmux is opt-in, not the default.**
  Per the user's 2026-10-19 request — "我本来的目的是支持
  tmux 会话的管理，不是所有 link 都要走 tmux" — `xscore link add
  <url>` now defaults to a `RawSink` (direct transport I/O, no tmux
  server in the loop). To get the full Link × Window × Pane tree
  plus `xscore data` ops, opt in with `?tmux=on` (mode=auto) or
  `?tmux=auto|local|remote` in the URL.

  Before (v0.1.0): every `xscore link add <url>` created a tmux
  session, with one default window + one default pane auto-created.

  After: `xscore link add <url>` creates a Raw link (only `link:`
  is printed). The `xscore window` / `xscore pane` subcommands
  return `Error::NotTmuxLink(<link_id>)` for raw links — add
  `?tmux=on` to use them.

  Schema: `SinkConfig` enum gained a `Raw` variant. Old state.json
  files with `SinkConfig::Tmux { ... }` deserialize unchanged
  (backward-compatible). New `Raw` links serialize as
  `"sink": "raw"`.

### Added

- `Error::NotTmuxLink(LinkId)` — returned by `add_window` /
  `add_pane` / etc. when the link uses a Raw sink.
- 6 URL parser tests for the new sink opt-in semantics.
- 3 cli_smoke tests: default = raw, raw link rejects `window add`,
  `?tmux=on` opt-in still creates window + pane.
- Column headers on all `list` commands: `xscore link list`,
  `xscore window list`, `xscore pane list`, `xscore link config list`.
- `TmuxSink::new_pane` race fix: 50ms sleep + `list-panes` after
  `split-window` to discover the authoritative pane id (workaround
  for tmux's server-internal async race where the returned pane
  id is sometimes stale).

## [Unreleased]

### Pending (deferred to v0.3.0+)

- `xscore link open` for the local-tmux case is a `todo!()` stub
  (manager.rs `open_link`). The ADR 0005 non-tmux invariant
  (`OpenOnlyForTmux`) IS wired and tested; the tmux-attach
  path lands in v0.2.0.
- SSH password auth + `known_hosts` (currently accepts any host
  key; password field not in `ConnectSpec` yet).
- `xscore config {show, path, set, unset}` for AppConfig (separate
  from the per-link `xscore link config` templates).
- Real `SerialTransport` and `SshTransport` end-to-end tests
  (current tests cover `ShellTransport` only; serial/SSH need real
  hardware / a real SSH server to exercise).
- Pre-built binaries (currently install via `cargo install xscore`).

## [0.1.0] - 2026-10-18

First public release. `xscore` is a Rust session manager for
SSH / shell / serial connections, modeled as **Link × Window × Pane**
per ADR 0006. This release ships the local-tmux + shell path
end-to-end; serial and SSH are real implementations of the
`Transport` trait but not exercised by the test suite (need
hardware / a real server to run end-to-end).

### Added

- **Workspace + CLI scaffold** (`0bcace9`, `f3c9493`)
  - Cargo workspace with the `xscore` binary crate.
  - `~/.config/xscore/{links,state}/` config layout.
  - `xscore --help` 3-level dispatch (subcommand groups + verbs).

- **Core types** (`e8723eb`, `c241610`)
  - `LinkId`, `WindowId`, `PaneId` (uuid v7) — `Debug + Clone + Copy + PartialEq + Eq + Hash + Serialize + Deserialize`. Round-trip via `to_string()` / `parse()`. Distinct types (compile error if you mix them).
  - `LinkAddress` (`Shell { command, args } | Serial { path } | Ssh { host, port, user, identity, proxy }`) for in-memory routing. NOT persisted (use `ConnectSpec` for that).
  - `core::Error` with 12 variants (`LinkNotFound`, `PaneNotFound`, `WindowNotFound`, `OpenOnlyForTmux`, `CloseOnlyForTmux`, `Url`, `Transport`, `Sink`, `Io`, `Config`, `Invalid`, `Serde`).

- **URL parser** (`754f1c9`)
  - `shell://<command>`, `serial:///dev/<path>?baud=...&data_bits=...&parity=...&stop_bits=...`, `ssh://<user>@<host>:<port>?identity=<path>`.
  - 5 tests: `parse_ssh_url_with_tmux`, `parse_shell_url_yields_shell_address`, `parse_serial_url_yields_serial_address`, `parse_rejects_unknown_scheme`, `ssh_with_different_identity_different_address`.

- **`Transport` trait + 3 implementations** (`754f1c9`, `64c3f6e`, `a095a13`)
  - `Transport` async trait: `kind`, `address`, `open`, `write`, `read`, `resize`, `close`. Object-safe.
  - `ShellTransport` — spawns a child process with piped stdin/stdout, reads/writes through. `cargo test` covers the `cat` echo round-trip.
  - `SerialTransport` (real, `serialport = "4.10"`) — opens a serial port at `path` with `baud`/`data_bits`/`parity`/`stop_bits`. Async wrapper around the sync `serialport` crate via `tokio::task::spawn_blocking`.
  - `SshTransport` (real, `russh = "0.43"`) — opens a TCP+SSH session, authenticates via key file (`?identity=~/.ssh/id_ed25519`), opens a session channel with `request_shell`. Read pulls from `Channel::wait()`; write goes through `Channel::data`. Resize via `window_change`.
  - `transport/factory.rs::transport_for(config)` dispatches on `TransportConfig` variant.
  - Limitations: serial `default-features = false` (no libudev; we open by explicit path only); SSH only key auth (no password); SSH accepts any host key (no `known_hosts`).

- **`Sink` trait + 2 implementations** (`754f1c9`, `40f7a27`)
  - `Sink` async trait: `kind`, `attach`, `new_window`, `kill_window`, `new_pane`, `close_pane`, `list_panes`, `snapshot`, `write`, `resize`, `close`.
  - `TmuxSink` — real wire to the local tmux server via `tokio::process::Command`. `new-session` / `new-window` / `split-window` / `kill-window` / `kill-pane` / `kill-session` / `capture-pane` / `send-keys` / `resize-pane`. Uses the shared `tmux -L xscore` server (ADR 0003) so multiple `xscore` processes share session state.
  - `RawSink` — no-op stub for `?tmux=off`. `new_window` is `unreachable!` (the manager routes around it for off mode).
  - `sink/factory.rs::sink_for(link_id, config, transport_kind, transport)` builds the sink, passing `link_id` to `TmuxSink` so it can populate `WindowInfo.link_id` on `new_window`.
  - `TmuxSink::adopt(link_id, mode, session_name)` — wraps an EXISTING tmux session without calling `new-session`. Used by hydration.

- **Registries + `SessionManager`** (`40f7a27`)
  - `LinkRegistry` (by_id + by_address secondary index), `WindowRegistry` (by_id + by_link), `PaneRegistry` (by_id), `BackendRegistry` (by_id → `Arc<dyn PaneBackend>`).
  - `SessionManager::add_link(spec, address) -> (LinkId, WindowId, PaneId)` — creates transport + sink + registers link + auto-creates 1 default window + 1 default pane (ADR 0006 ergonomic: the common path stays one step).
  - `add_window(link_id, name) -> WindowId` — `tmux new-window` + register.
  - `add_pane(window_id, name) -> PaneId` — `tmux split-window` + register + create backend.
  - `del_link` / `close_link` / `del_window` / `del_pane` cascading cleanup.
  - `backend(pane_id) -> Arc<dyn PaneBackend>` for the data subcommand.
  - ADR 0005 invariants: `close_link` rejects non-tmux transports with `CloseOnlyForTmux`; `open_link` (currently a `todo!()` for tmux) rejects non-tmux with `OpenOnlyForTmux`.

- **Backends** (`40f7a27`)
  - `PaneBackend` trait: `snapshot`, `write`, `write_raw`, `resize`, `close`.
  - `BackendRegistry` (pane_id → `Arc<dyn PaneBackend>`).
  - `CompositeBackend` — delegates `snapshot`/`write`/`resize` to the link's sink (via `sink_handle`) and `write_raw` to the link's transport.

- **CLI** (`496d49f`, `c7ab025`)
  - 4-verb `link` tree (ADR 0005): `add` / `open` / `close` / `del` + `show` + `list` + `config {add,remove,show,list}`.
  - 4-verb `window` tree (ADR 0006): `add` / `del` + `show` + `list`.
  - 4-verb `pane` tree: `add` (`--window` parent) / `del` + `show` + `list`.
  - `data {read, write, resize}` over `pane-id` (task-11).
  - `config` (stub for now; task-12).
  - `resolve_link_input` helper: URL / `-c CONFIG` / URL+`-c CONFIG` precedence.
  - `merge_specs` for the URL+template combo.
  - Templates persisted at `~/.config/xscore/links/<name>.json`.

- **State persistence** (`c2d4407`)
  - `~/.config/xscore/state/state.json` — single-file snapshot of `Vec<LinkSnapshot { link, windows, panes }>`.
  - Atomic write (`.tmp` + rename) for crash safety.
  - `SessionManager::save_state()` called after every mutating method.
  - `SessionManager::load_from_state()` on startup — re-queries the tmux server (adopts existing sessions via `TmuxSink::adopt`), uses `NoopTransport` for I/O (the user can re-add the link to get a fresh transport for data ops), re-registers each window + pane with its original xscore id.
  - Best-effort hydration: links whose tmux session is gone are skipped.

- **Tests** (`8baa55d`, `c2d4407`, `0cc8ac5`)
  - 14 unit tests across `core`, `url`, `sink`, `transport`.
  - 11 CLI integration tests (`tests/cli_smoke.rs`) — each test uses a unique `HOME` to isolate state across parallel runs.
  - 3 data-path integration tests (`tests/data_smoke.rs`) — single-process via `SessionManager` API, real tmux round-trips.
  - Total: **28 tests, 0 ignored, 0 failures**.

### Changed

- **CLI** — `xscore link add` now prints all three IDs: `link:`, `window:`, `pane:` (was just `link:` and `pane:` — the test extractor needs the window id too).
- **CLI** — `eprintln!` for `link:` switched to `println!` (program output goes to stdout; stderr is for diagnostics).
- **`manager::open_link`** — non-tmux case now returns `Error::OpenOnlyForTmux(<kind>)` cleanly (was `todo!()` panic).
- **`TmuxSink::new_window`** — uses `#{window_index}` not `#{window_id}` (the latter is a STABLE unique id that increments globally across the tmux server, not the 0-based position; using it as an index caused `split-window -t <session>:<idx>` to fail on the 2nd+ link added in a session).
- **`sink/factory.rs::sink_for`** — session name = `xscore-<link_id>` (was a random uuid per call). The manager needs to derive the session name on hydration to find it in tmux.

### Fixed

- **Double-default-window bug** (`0cc8ac5`) — `xscore link add` was producing 2 tmux windows per link (1 from `TmuxSink::create`'s `new-session`, 1 from `manager::add_link`'s `add_window`). Now produces 1. Regression test: `add_link_creates_exactly_one_window`.
- **Tmux window index drift** (`a095a13` via `754f1c9`) — `#{window_id}` confused for index. Now uses `#{window_index}` (integer, position-based).
- **CLI `link:` on stderr** (`8baa55d`) — tests parsed stdout for `link:`, but it was on stderr. Switched to `println!`. Test pattern matches.
- **`Uuid::parse → Uuid::parse_str`** (`c241610`) — uuid 1.10 split the API; spec example used the old name.

### Known limitations (deferred, not blocking v0.1.0)

- **`xscore link open` for tmux** — non-tmux paths reject cleanly with `OpenOnlyForTmux`; the tmux-attach path is a `todo!()` (lands in v0.2.0).
- **No `known_hosts` for SSH** — any host key is accepted. Real users want `StrictHostKeyChecking=yes` with a `~/.ssh/known_hosts` lookup.
- **No SSH password auth** — only key-based. The `ConnectSpec::Ssh` doesn't carry a `password` field yet.
- **Serial `available_ports()` enumeration** — `default-features = false` skips `libudev`. Future `xscore link serial list` (a proposed verb) would need either `libudev-dev` or `/sys/class/tty/*` scanning.
- **Hydration uses `NoopTransport`** — `data read` / `data write` on a loaded link fail cleanly until the user re-runs `xscore link add <url>` (the original child process / SSH channel doesn't survive a process restart). Documented in README.
- **State migration** — the `tmux_window_index` persisted before commit `0cc8ac5` (the double-window fix) is 1; new links use 0. Users with old state files should `xscore link del` + re-add to migrate.
- **No prebuilt binaries** — install via `cargo install xscore --path .` (from the workspace root). Future: GitHub Releases artifacts.

[Unreleased]: https://github.com/loner/xscore/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/loner/xscore/releases/tag/v0.1.0
