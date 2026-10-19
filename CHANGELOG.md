# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

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
