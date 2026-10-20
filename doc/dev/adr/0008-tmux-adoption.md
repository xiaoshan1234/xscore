# ADR 0008: tmux adoption, real pane buffers, and `link add` auto-create

- **Status:** Accepted
- **Date:** 2026-10-20
- **Deciders:** dev
- **Supersedes in part:** [ADR 0007](./0007-raw-default-tmux-opt-in.md) —
  creation semantics only ("no auto window/pane" → "auto-create +
  tmux adoption"). The 3-entity model itself is unchanged.
- **Related:** [ADR 0002](./0002-transport-sink-split.md) (Sink stays
  optional), [ADR 0003](./0003-external-tmux-server.md) (still the
  shared `tmux -L xscore` server, still external)

## Context

v0.3.0 shipped a decoupled 3-entity model but the runtime behind it was
not honest:

1. `link add` produced a link with **no way to do I/O**. Every user had
   to run `pane add` + `pane attach` before `data read/write` worked —
   for tmux links that meant xscore guessed which split to talk to.
2. `detach_pane` called `DirectBridge::close()`, which was
   `transport.close()`: detaching one pane **killed the link's
   transport**. The link stayed in the registry but was dead. Re-attach
   produced a bridge over a closed child.
3. `DirectBridge` had **no buffer**. `read` was a one-shot
   `transport.read()`, so "the pane's core is a buffer" was a comment,
   not an implementation, and "detach keeps the buffer" was false.
4. Neither `PaneInfo` nor `WindowInfo` recorded tmux's identity for the
   thing it mirrored. After a restart there was no way to map a
   `PaneId`/`WindowId` back to `%N`/`@N`, so any reconciliation between
   xscore and tmux was impossible.
5. `state.json` carried **no schema version**. A v0.3.0-era file (or a
   much older one) deserialized "successfully" into a half-empty
   snapshot: the user's links silently vanished.

The underlying design error was that xscore tried to *predict* tmux's
structure (pane index, window index, geometry) and then reconcile later.
tmux is already the authority for all of it.

## Decision

### 1. tmux is the source of truth for structure

xscore never invents a tmux pane/window id and never guesses geometry.
Every structural value comes from a tmux query:

| xscore action | tmux query |
|---|---|
| adopt a link's window | `list-windows -t <session> -F '#{window_id}\t#{window_name}\t#{window_width}\t#{window_height}\t#{window_layout}'` |
| adopt/split a pane | `list-panes -t <window> -F '#{pane_id}\t#{pane_width}\t#{pane_height}\t#{pane_left}\t#{pane_top}\t#{pane_index}'` |
| create a window | `new-window -t <session> -n <name> -P -F …` |
| split a pane | `split-window -t <window> -P -F '#{pane_id}'` + `list-panes` |
| delete | `kill-pane -t %N`, `kill-window -t @N`, `kill-session -t <name>` |

`cols`/`rows`/`x`/`y` in `PaneInfo` and `cols`/`rows` in `WindowInfo` are
copies of what tmux reported at adoption time, refreshed by `link sync`
(or `--heal`). `Window.layout` is a best-effort hint parsed from tmux's
raw layout string (`even-horizontal` → `split_h`, `even-vertical` →
`split_v`, `tiled` → `grid`, anything else → `single`).

### 2. `%N` and `@N` are the identity anchors

`PaneInfo.sink_handle: Option<String>` holds `"%3"`;
`WindowInfo.tmux_window_handle: Option<String>` holds `"@0"`. `None`
means the entity is virtual (xscore-only).

The numeric *index* (`#{pane_index}`, `#{window_index}`) is display
data and shifts whenever neighbours are created or destroyed. It is
never used for identity, and never persisted as a key.

Consequence: a hydrated manager can rebuild a `TmuxBridge` from
`(sink_handle, link)` alone — no re-attach, no new split, no guessing.

### 3. `link add` always yields a usable pane, in one of three ways

```
link add <url>                      → link + virtual window + pane (ring buffer)
link add '<url>?tmux=on'            → link + adopted tmux window + adopted tmux pane
link add <url> --pane <pid>         → link + that existing pane (no window created)
```

Rules:

- `--pane` with a tmux link is an error (`TmuxLinkCannotBindPane`): tmux
  panes are created by tmux, and binding a hollow pane to a session it
  isn't part of has no coherent meaning.
- `--pane` with a pane that is already attached is an error
  (`PaneAlreadyAttached`). We never silently steal a pane from another
  link; the user detaches first.
- `--pane` with a tmux-owned pane is also `TmuxLinkCannotBindPane`.
- The direct `link add` branch creates a **virtual** window
  (`tmux_window_handle: None`, 120×30) and a pane (80×24) with a
  `DirectBridge`.

### 4. `pane add --window <wid>` is the only way to grow a tmux window

A pane is split inside an explicit tmux window (`--window` required);
there is no implicit window 0. If the target window has a
`tmux_window_handle`, xscore finds the owning link (by asking each tmux
link for its window list — not by parsing session names, which can be
overridden with `?name=`), splits there, and stores the returned `%N`
plus geometry. Otherwise the pane is hollow.

`window add --link <tmux-lid>` creates a real tmux window and adopts
it; `--link <direct-lid>` is `NotTmuxLink`.

### 5. `detach` releases a hold; it never kills a shared resource

- `DirectBridge::close()` stops the per-pane reader task and does
  **not** touch the transport. The link closes it exactly once, in
  `del_link`.
- `TmuxBridge::close()` is a real `kill-pane` — that tmux pane is this
  pane's alone.
- `detach_pane` refuses for tmux panes (`TmuxPaneNotDetachable`): they
  are tmux's, so the way to get rid of one is `pane del` (which kills
  the tmux pane).

`link del`:

- direct link → panes become orphans (`link_id = None`), buffers kept;
  transport closed once.
- tmux link → `kill-session`; the adopted windows and panes are removed
  from the registry with the session (no per-pane `kill-pane`).

### 6. The pane's core is its buffer

`DirectBridge` owns a `RingBuffer` (64 KiB) fed by a background reader
task, and `read` serves a **non-destructive snapshot** of it (tmux's
`capture-pane` semantics, not a consuming stream). The buffer lives in
the manager keyed by `PaneId`, so a detached pane keeps its history and
a re-attach continues appending to it.

The reader task polls `transport.read()` under a short timeout
(`READ_POLL = 10 ms`) instead of blocking indefinitely. Reason:
transports are allowed to serialize read/write behind a single lock
(`ShellTransport` does), so a blocking reader would starve `write`
forever. Cancelling a pipe read is safe, so the timeout is a cheap way
to yield the lock while keeping the pump architecture.

### 7. `state.json` is versioned, and mismatches are fatal

```rust
pub const SCHEMA_VERSION: u32 = 2;
```

`load()` probes `schema_version` first and refuses anything else:

```
state.json schema_version=0, expected 2; delete ~/.config/xscore/state/state.json to reset
```

Missing → treated as `0` → rejected. No serde default on the field, so a
stale file can never masquerade as current. Silent degradation is worse
than an actionable error.

## Alternatives considered

- **Keep `link add` lean, add a `--with-pane` flag.** Keeps v0.3.0
  behaviour but leaves the default path useless and the docs apologetic.
  Rejected: a link without a pane cannot do anything.
- **Track tmux by window/pane index.** Rejected: indexes are positional.
  `kill-window` on a neighbour silently re-points every index at the
  wrong object.
- **Let xscore create tmux panes itself and remember the ids.** Rejected:
  duplicates tmux's bookkeeping and breaks the moment a user touches
  tmux directly.
- **Silently migrate old state.** Rejected for this release (removed
  fields make the mapping lossy); an explicit error with a one-line fix
  is better.
- **Streaming (draining) buffer reads.** Rejected: a detached and
  re-attached pane would lose the screen it was showing. Scrollback
  semantics match how a terminal pane is used.

## Consequences

- `link add` is now a one-shot "give me something I can type into",
  which is what the CLI is for.
- tmux and xscore can drift (a user creates a pane in tmux directly).
  `link sync --id <lid>` reports that drift; `--heal` corrects registry
  rows from tmux and drops rows whose tmux object is gone. Panes that
  tmux has but xscore never adopted are **reported only** — adoption is
  an explicit, user-driven act, never a background side effect.
- Direct panes cannot survive a process restart with live I/O (their
  transport died with the process). tmux panes can: the `%N` handle is
  enough to rebuild the bridge on hydrate.
- v0.3.0 `state.json` files must be deleted on upgrade.

## Implementation map

| Concern | Where |
|---|---|
| tmux queries + snapshots | `sink/tmux.rs`, `sink/mod.rs` (`TmuxWindowSnapshot`, `TmuxPaneSnapshot`, `layout_from_tmux`) |
| ring buffer | `backends/ring_buffer.rs` |
| `DirectBridge` pump / `TmuxBridge` | `backends/bridge.rs` |
| creation branches, `del_link`, `detach`, `sync_link` | `core/manager.rs` (`LinkAddResult`, `SyncReport`) |
| schema gate | `core/state.rs` (`SCHEMA_VERSION`, `parse_snapshot`) |
| CLI | `cli/link.rs` (`add --pane`, `sync`), `cli/window.rs` (`add --link`), `cli/pane.rs` (`add --window`) |
| errors | `core/error.rs` (`TmuxLinkCannotBindPane`, `PaneAlreadyAttached`, `TmuxPaneNotDetachable`) |
