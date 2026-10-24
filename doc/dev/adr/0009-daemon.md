# ADR 0009: a resident daemon for direct-link transports and pane buffers

- **Status:** Accepted
- **Date:** 2026-10-20
- **Extends:** [ADR 0008](./0008-tmux-adoption.md) — the binding model,
  the Q1 rule and the buffer semantics are unchanged; only *where the
  byte-level objects live* changes.
- **Related:** [ADR 0002](./0002-transport-sink-split.md) (transport owns
  the connection), [ADR 0007](./0007-raw-default-tmux-opt-in.md) (raw is
  the default sink)

## Context

`link add shell:///bin/bash` creates a `Transport` — a child process, a
serial fd, an ssh channel — inside the CLI process that ran it. That
process then exits.

Everything downstream of that was structurally impossible:

```
$ xscore link add shell:///bin/cat
link: ...   window: ...   pane: ...
$ xscore data write --pane <pid>
Error: ... not connected (its transport died with the previous process)
```

v0.4.1 made this honest (a separate `connected` flag plus
`Error::LinkNotConnected`, so a bound-but-dead link no longer pretended
the pane was never attached), but honest is not usable. The pane's ring
buffer had the same problem: it lived in `pane_buffers` in the CLI
process, so a pane's scrollback evaporated when the command returned.

tmux links never had this problem, and that is the tell: tmux is a
resident server, so a tmux link's transport and scrollback outlive the
CLI by construction. Direct links had no such home.

The question was therefore not *whether* something must be resident, but
what it should own, and how much of the model it should take over.

## Decision

### 1. One daemon serves every link, not one daemon per link

```
daemon process holds:
  links: HashMap<LinkId, Arc<dyn Transport>>
  panes: HashMap<PaneId, PaneSlot { link: Option<LinkId>,
                                    buffer: Arc<Mutex<RingBuffer>>,
                                    pump: Option<JoinHandle<()>> }>
```

A pane is freely movable: `pane attach --pane P --link B` after it was
on link A. Its **buffer must follow it**. A per-link daemon would mean
the buffer either lives with the wrong link or gets copied across a
process boundary on every re-bind — copies that race, lose bytes, or
both. With one daemon, re-binding a pane is a pointer change
(`slot.link = Some(B)`) plus starting a pump.

The cost is blast radius: one daemon crash takes every direct link down.
That is accepted, because the failure is *observable and cheap* — links
report `connected: false`, `pane attach` re-creates the transport from
persisted metadata, and nothing in `state.json` is damaged.

### 2. The CLI keeps the metadata; the daemon only owns bytes

| | owner | survives CLI exit | survives daemon exit |
|---|---|---|---|
| Link / Window / Pane records, ids, bindings | CLI (`state.json`) | yes | yes |
| live `Transport` objects | daemon | yes | no |
| live tmux control clients (v0.6.0) | daemon | yes | no |
| pane `RingBuffer`s (native and tmux) | daemon | yes | no |

The CLI stays the authority for *what exists and what is bound to what*.
The daemon is a byte-level cache with a lifetime. Consequences:

- `link list`, `pane show`, `window show` work with the daemon stopped
  (they never needed a transport).
- `link del` still orphans panes and rewrites `state.json` even if the
  daemon is unreachable.
- Killing the daemon is not data loss. It costs a reconnect.
- The daemon never parses or writes `state.json` — there is exactly one
  writer of that file, so the two processes cannot disagree about it.

### 3. tmux links do not go through the daemon

> **Superseded in v0.6.0 (ADR 0010 §D2, PR5).** This section used to say a
> managed tmux session was driven by `TmuxBridge` → one `tmux` command per
> operation, and that the daemon was involved only for `SinkConfig::Raw` links.
> Both objects are gone (ADR 0010 §D7 removed the sink; PR4 removed the
> per-command path), and the daemon now holds the control client itself. The
> rule the old text encoded still holds where it matters — the daemon is not a
> hop that re-marshals bytes — and the corrected statement is:

The daemon **now owns tmux control clients as well as native transports.** A
managed session's single `tmux -C attach` client, its `%output %N`
subscriptions and every pane buffer live in `ControlSlot`s — one per session,
never one per pane or per link — so `data read` in a later CLI process returns
bytes rather than a screen snapshot, and the client is not re-spawned for every
command. Consequences: **idle exit is gated on both** (no links *and* no control
slots, 5 s of quiet), `daemon stop` closes the clients and therefore drops the
buffers (they are memory, like a native pane's), and `session kill` /
`session detach` send `CloseTmux` first — the one-shot `kill-session` still
runs outside the daemon for `kill`. The reason for the old rule is unchanged:
tmux is already a resident server, so the daemon is not a hop in the byte path
that re-marshals or copies anything — it is a second resident *holder* of the
same single connection.

### 4. Protocol: length-prefixed JSON over a Unix socket

`[u32 little-endian length][JSON UTF-8]`, requests tagged by `op`:

```json
{"op":"ping"}
{"op":"status"}
{"op":"create_link","link":"<uuid>","transport":{"transport":"shell","command":"/bin/bash","args":[],"cwd":null}}
{"op":"delete_link","link":"<uuid>"}
{"op":"create_pane","pane":"<uuid>"}
{"op":"delete_pane","pane":"<uuid>"}
{"op":"attach","pane":"<uuid>","link":"<uuid>"}
{"op":"detach","pane":"<uuid>"}
{"op":"read","pane":"<uuid>","max":4096}
{"op":"write","pane":"<uuid>","data":"<hex>"}
{"op":"resize","pane":"<uuid>","cols":120,"rows":30}
{"op":"stop"}
```

```json
{"ok":true}
{"ok":true,"data":"<hex>"}
{"ok":true,"n":13}
{"ok":true,"status":{"pid":123,"links":["<uuid>"],"panes":[{"pane":"<uuid>","link":"<uuid>|","buffered":1234}]}}
{"ok":false,"error":"..."}
```

- **JSON, not a binary codec.** The payloads are tiny (a keystroke, a
  screen snapshot) and the request set is a dozen ops. JSON keeps the
  protocol inspectable with `socat`/`nc` and lets `daemon status` print
  the struct it parsed.
- **The transport is `model::TransportConfig`'s own serde shape**
  (`#[serde(tag = "transport")]`), which is the same shape `state.json`
  persists. There is no second transport vocabulary to keep in sync.
- **`data` is hex.** Base64 would need a crate (or a hand-rolled
  encoder with padding rules) to save ~25% on payloads that are already
  small; `format!("{:02x}")` / `char::to_digit(16)` is 20 lines and
  unambiguous. No new dependencies.
- **One connection per request** on the client side. A CLI invocation
  makes a handful of requests; a stateless connection has no broken-pipe
  state to recover after a daemon restart. The server loops on a
  connection until EOF, so a future long-lived client (a TUI) can reuse
  one.
- **v0.6.0 adds five ops for tmux** (`open_tmux`, `structure`,
  `tmux_command`, `close_tmux`, `probe_tmux`), and `attach` grew optional
  `key`/`session` fields so one request covers both "pane on a pipe link" and
  "pane `%N` on a session's control client". `structure` and `probe_tmux` exist
  because the client is resident: a listing is one query, not a spawned process,
  and liveness is a flag the daemon already keeps.

### 5. Lifecycle: auto-start, idle exit, explicit stop

- **Socket:** `$XDG_RUNTIME_DIR/xscore/daemon.sock`, else
  `$HOME/.config/xscore/run/daemon.sock`. Never a fixed `/tmp` path:
  two users or two test HOMEs must never share a control channel.
  (Unix `sun_path` is ~104 bytes, so a deep `HOME` is the one real
  constraint — the `XDG_RUNTIME_DIR` branch exists for exactly that.)
- **Auto-start:** any command that needs a transport (`link add`,
  `pane attach`) connects; if nothing answers it spawns
  `xscore daemon serve` detached (`current_exe`, stdio closed) and polls
  the socket every 20 ms for up to 2 s. The user never starts it.
- **Single instance:** the daemon takes a lock file (`create_new`) before
  binding. Without it, two CLI processes auto-starting at the same moment
  could each unlink-and-bind, leaving one daemon listening on an orphaned
  inode — two live daemons, each holding some of the links, forever.
  A lock whose socket no longer answers is treated as stale and taken
  over (same for a stale socket file: connect, and if it fails, unlink
  and rebind).
- **Idle exit:** with **no links and no tmux control slots** and 5 s without a
  request, the daemon exits and removes its socket and lock. A daemon holding a
  link or a control client stays up — that connection is the entire reason it
  exists. This is what keeps test suites and one-off commands from leaving
  processes behind. (A managed tmux session therefore keeps the daemon alive by
  design; `daemon stop` is the way out, and it closes those clients.)
- **Explicit stop:** `xscore daemon stop` → `{"op":"stop"}`, answered
  before exit so the client's request is not a broken pipe.

### 6. Pumps, and the 10 ms poll

Each attached pane has one reader task inside the daemon:

```rust
match timeout(10ms, transport.read(&mut buf)).await {
    Ok(Ok(0)) => sleep(10ms),
    Ok(Ok(n)) => buffer.lock().await.push_slice(&buf[..n]),
    Ok(Err(_)) => break,
    Err(_)     => {}          // timed out: re-arm
}
```

This is v0.4.1's `DirectBridge` loop, unchanged and deliberately so:
transports serialise `read`/`write` behind one lock (`ShellTransport`
does), so a blocking `read` would starve `write` forever. Cancelling a
pipe read loses no bytes. Serial's own 5 ms port timeout is shorter than
the 10 ms poll and stays as it is.

`read` serves a **non-destructive snapshot** of the buffer and waits up
to 200 ms (10 ms steps) for bytes to appear, exactly like
`DirectBridge::read` — the CLI's observable behaviour is unchanged.

### 7. The Q1 rule is enforced in both places

`pane attach` is the CLI's job (metadata), but the daemon must enforce
"one active pane per direct link" as well: two pumps on one transport
would duplicate every byte into two buffers and interleave writes. So
`Attach` first detaches any other pane on that link (aborting its pump,
keeping its buffer) — the same rule, the same outcome, one lock-free
implementation per process.

### 8. Reading a detached pane is allowed; writing is not

A pane that is detached keeps its slot in the daemon, so `data read`
still returns its scrollback (a pane's core is its buffer, ADR 0008) —
this is what makes "detach, then read in another process" work. `write`
and `resize` require an attached link and fail with the same
"not attached" text they always did. The asymmetry is deliberate: reads
are snapshots of owned history, writes need a live peer.

## Testability

The daemon is the first part of xscore whose whole job is to cross a
process boundary, so the tests must cross one too:

- `cargo test --test cli_smoke` drives the real binary, one process per
  step: create in A, write in B, read in C. `xscore_daemon_direct_link_survives_process`
  is the acceptance test — it is exactly the sequence that was
  impossible before.
- Each test gets its own `HOME`, so it gets its own socket
  (`.../xscore/run/daemon.sock`). `XDG_RUNTIME_DIR` is removed from the
  child environment so the documented HOME fallback applies and no two
  tests can share a daemon.
- `cleanup_daemon()` stops the test's daemon at the start and end of the
  daemon tests; the 5 s idle exit covers everything else.
- The daemon's own logic (`Attach`/`Detach`/`DeleteLink`/`Read`/`Write`,
  Q1 enforcement, buffer retention) is unit-tested in-process against a
  scripted `Transport` — no socket, no timing.

## Alternatives considered

- **Keep it in-process and document the limitation.** Rejected: it makes
  `link add` followed by `data write` — the only obvious workflow —
  impossible, for every user, always.
- **One daemon per link.** Rejected: the buffer cannot follow a pane
  across daemons without copying, and a pane switching links is a normal
  operation (ADR 0007).
- **Let the daemon own `state.json`.** Rejected: it would make every
  read-only command (`link list`) depend on a running daemon, and put the
  model's only writer behind a socket.
- **Hold the daemon open by keeping a client connection alive.** Rejected
  as the *only* lifetime mechanism: the daemon would die with whichever
  terminal started it. Pane/reference counting plus idle exit is
  simpler to reason about.
- **A pipe or FIFO per link instead of a socket.** Rejected: the
  protocol has request/response pairs (status, read) and needs
  multiplexing; a FIFO gives neither without inventing framing addresses.
- **Persist pane buffers to disk so they survive a daemon restart.**
  Rejected for this release: scrollback is worth seconds-to-minutes of
  re-creation, not a durability contract with fsync, retention and
  cleanup rules.

## Consequences

- `xscore link add shell://...` then `xscore data write --pane ...` in
  another terminal now works. Direct links are as persistent as tmux
  links.
- A pane's scrollback (64 KiB) survives the CLI, so `pane detach` →
  `data read` shows what was on screen.
- `link show`'s `connected` is now a **live probe** of the daemon
  (`{"op":"status"}`) rather than a flag set at hydration. It costs one
  round trip on `link show` only.
- One more process to reason about. Mitigated by: auto-start (no setup),
  idle exit (no leftovers), `daemon status` (one JSON line per link and
  pane), `daemon stop` (one command), and the CLI's metadata-first
  design (a dead daemon never corrupts state).
- `pane attach` doubles as a reconnect: if the daemon lost the
  transport, the manager re-creates it from the persisted
  `TransportConfig` and binds again.
- Direct links still cannot survive a *daemon* restart with live I/O.
  That is a smaller loss than before (metadata, bindings and buffers are
  unaffected; re-attach reconnects) and is the honest limit of this
  design.

## Implementation map

| Concern | Where |
|---|---|
| wire format, hex, framing | `daemon/protocol.rs` |
| socket bind, lock, accept loop, pumps, Q1, status | `daemon/server.rs` |
| auto-start, requests, `DaemonTransport` | `daemon/client.rs` |
| CLI-facing bridge | `backends/bridge.rs` (`DaemonBridge`) |
| daemon-backed link creation / attach / detach / del | `core/manager.rs` (`enable_daemon`, `link_is_connected`, `bridge_for`) |
| `daemon serve/status/stop` | `cli/daemon.rs`, `cli/mod.rs` |
| error surface | `core/error.rs` (`Error::Daemon`, `LinkNotConnected`) |
