# ADR 0010: Session owns the backend; tmux is driven through control mode

- **Status:** Accepted (2026-10-21)
- **Date:** 2026-10-21
- **Deciders:** dev
- **Related:** [ADR 0001](./0001-link-pane-model.md), [ADR 0002](./0002-transport-sink-split.md), [ADR 0003](./0003-external-tmux-server.md), [ADR 0006](./0006-window-concept.md), [ADR 0007](./0007-raw-default-tmux-opt-in.md), [ADR 0008](./0008-tmux-adoption.md), [ADR 0009](./0009-daemon.md)
- **Design:** [arch.md](../arch/arch.md) — the full target design (sections 0–13)

## Context

v0.5.1 has links, windows, panes and a daemon, and three structural problems.

**1. tmux-ness was stored on the link.** `SinkConfig::{Raw, Tmux}` plus
`?tmux=on|auto|local|remote` made "does this go through tmux" a property of a
*connection*. Three symptoms, all verified in the code:

- `?tmux=remote` was parsed and never read (`grep -n "self.mode" sink/tmux.rs`
  returned nothing), so remote tmux silently created a **local** session;
- a tmux link opened a transport for the command in the URL and then ignored it
  — `link add 'shell:///bin/bash?tmux=on'` started two shells, and
  `?tmux` never ran the requested command;
- `link sync --heal` existed solely to reconcile xscore's copy of tmux's
  structure with tmux itself.

**2. tmux was driven per command.** `split-window`, `capture-pane`, `send-keys`
— one exec each. So a pane read was a *rendered screen snapshot*, structure
changes could only be discovered by polling, and a write had to survive `argv`
on the way to a shell.

**3. There was no workspace.** Links, windows and panes floated with no owner;
"which links belong to this tmux session" was inferred from a name prefix
(`xscore-<link_id>`), which the user can override.

## Decision

### D1 — Session is the workspace, and it owns the backend

```rust
pub struct SessionInfo {
    pub id: SessionId,
    pub name: String,
    pub created_at: DateTime<Utc>,
    pub backend: SessionBackend,
}

pub enum SessionBackend {
    Native,
    Tmux {
        host: TmuxHostConfig,     // Local | Ssh { destination, args }
        socket: TmuxSocket,       // Default | Named | Path
        session: String,          // tmux session name, user-visible
    },
}
```

A session holds **N** windows, **N** panes and its links. `session kill` is the
only cascade in the model. "Does this go through tmux?" becomes a question about
the workspace, which is where it always belonged.

### D2 — tmux is reached through control mode, with one link per session

```
tmux -L <sock> -C attach -t <session>           # local
ssh <args> <dest> tmux -L <sock> -C attach -t <session>    # remote
```

That process is the session's **single link**: the connection to the *server*,
not the connection of a pane.

```
Link (Control) ── bytes ──► Sink::dispatch
                             ├── %output %N          → pane %N's RingBuffer
                             ├── %window-add, %layout-change, %exit → state
                             └── %begin/%end          → the reply to a command
```

Measured on tmux 3.5a with a private socket, not assumed:

| fact | evidence |
|---|---|
| a pane is a byte stream | `%output %0 TICK\015\012` |
| escapes are octal, `\\` is a backslash | `%output %1 \033[?2004h…` |
| one line is one record | newline appears only as `\012` |
| structure is pushed | `%window-add`, `%layout-change`, `%session-window-changed` |
| commands are framed | `%begin <ts> <num> <flags>` … `%end` |
| writes are literal | `send-keys -l` delivered `;` `"` `$` and backticks unchanged |
| nothing is replayed | output before attach is absent from the stream |

`-C`, not `-CC`. Measured on 3.5a: `-CC` disables echo by calling
`tcgetattr` on the client's terminal, so a pipe client dies at once —
`tcgetattr failed: Inappropriate ioctl for device`. The same pipeline with `-C`
returns `%begin … @0 … %end`. `-CC` is for clients that own a real terminal
(iTerm2 allocates a pty); a pipe client wants `-C`.

### D3 — Exclusivity is per link kind

| link | panes it serves | how a pane addresses itself |
|---|---|---|
| `Pipe` (shell / serial / ssh) | at most **one** | it does not — it is the only reader |
| `Control` | **all** panes of its session | `link_key = "%N"` |

The blanket rule "no link may be shared" was true of a raw byte stream (two
readers steal each other's bytes) and false of a protocol that names its
recipients. Sharing here is **addressing**, not a race.

### D4 — Sink returns, as a per-link demultiplexer

`capacity` / `grant` / `revoke` stay gone: tmux assigns the keys itself, so
there is nothing to grant. What returns is `dispatch`:

```rust
pub trait Sink: Send + Sync {
    fn dispatch(&self, chunk: &[u8]) -> Vec<Dispatch>;
}
pub enum Dispatch {
    Data { key: Option<String>, bytes: Vec<u8> },  // key None = the only pane
    Event(TmuxEvent),
}
```

`PipeSink` forwards one `Data{key: None}` per read. `ControlSink` keeps a
carry-over line buffer (a read can split a line; a screen redraw is one long
`%output`), routes `%output` to panes, turns the rest into events, and drops
unknown notifications — tmux renames them across versions (3.5a emits
`%unlinked-window-close` where older releases emitted `%window-close`).

### D5 — Window mirrors `@N`; layout belongs to tmux

For a tmux-backed session, `WindowInfo` is a projection of a tmux window and the
layout string is read from tmux when needed. A private xscore grouping would
contradict what `tmux attach` shows. `WindowLayout` — a five-variant enum with no
consumer — is deleted; native windows keep no layout model until a renderer
exists.

### D6 — Pane carries a key, not a backend

`PaneBackend` is deleted. A pane's kind is its **link's** kind, so storing it
twice invites "this pane is a tmux pane but its link is a pty". The pane keeps
`link_id` and `link_key` (`"%N"` for a tmux pane, `None` for a pipe link).

### D7 — Removed

| removed | because |
|---|---|
| `SinkConfig::{Raw, Tmux}` | tmux-ness is a session property |
| `?tmux=on\|auto\|local\|remote` | same, and it never worked for `remote` |
| `Sink::capacity` / `grant` / `revoke`, `RawSink`, `TmuxSink: Sink` | keys are assigned by tmux |
| `Link.active_pane` (Q1 ledger) | a `Pipe` link has one reader; a `Control` link addresses by key |
| `link open` / `link close` | `session attach` / `session kill` / `session forget` |
| `link sync [--heal]` | events replace reconciliation |
| `WindowLayout` | no consumer |
| `WindowInfo.tmux_window_handle`, `PaneInfo.sink_handle`, `PaneBackend` | folded into `WindowBackend::Tmux` and `link_key` |
| `backends/` | `sink/` (demux) and `core/buffer.rs` (the pane's buffer) |

### D8 — State schema v3

Adds `sessions`; every child carries `session_id`; `sink` / `sink_handle` /
`tmux_window_handle` / `PaneBackend` disappear; panes gain `link_key`. v2 files
are rejected with an actionable error, not silently converted (same policy as
v0.4.0). Migration is mechanical: each existing tmux link becomes a
`Session{Tmux}` whose structure is adopted and whose panes get `link_key = %N`;
each existing raw link becomes a `Session{Native}`.

## Consequences

- **`link del` is refused for a `Control` link.** Every pane in the session
  routes through it; deleting it would leave them with buffers and no way back.
  Teardown is `session kill` (destroy) or `session forget` (stop managing).
- **`link add` is rejected inside a tmux session**, and `link ls --session work`
  prints exactly one row. There is one connection to a server.
- **`pane new` in a tmux session no longer creates a link** — it inserts a pane
  row with `link_key = %N` on the existing link.
- **The control client is a per-session single point of failure.** If it dies,
  every pane in that session stops streaming at once. tmux keeps running and
  re-attaching resumes, but the gap is lost.
- **`capture-pane` is demoted to `pane screen`.** "The rendered grid" and "the
  bytes since I last looked" are different questions; only the second is `data`.
- **Remote tmux is one ssh channel**, not one exec per command, so
  `ControlMaster` is only needed for one-shot queries.
- **The control link must be a dedicated exec**, never an existing interactive
  shell link: a prompt or echo sharing the stream is exactly the failure mode
  that made `ThroughTransport` unworkable. For connection reuse, use OpenSSH
  `ControlMaster`.
- **Native shells still need a pty.** Nothing in this ADR fixes `Stdio::piped()`
  in `ShellTransport`; that is a prerequisite for interactive native panes
  ([arch.md, "Behaviour that must change"](../arch/arch.md#behaviour-that-must-change-defects-not-features)).

## Alternatives rejected

1. **One link per tmux pane (`Screen` endpoint).** This was the intermediate
   design. Rejected: it materialises a link row that can never be re-bound or
   reused, and it hides the 1:N multiplexing that control mode actually provides.
   The link is the connection to the *server*.
2. **`capture-pane` as the read path.** Loses ordering, has no notion of a
   stream, and forces polling for structure. It cannot express "bytes since the
   last read".
3. **`ThroughTransport`: run tmux commands over an existing `Transport`.** No
   framing of our own — prompts, banners and partial writes corrupt the stream.
   Control mode is different in kind, because the framing (`%begin`/`%end`,
   distinct `%output` namespace) is part of tmux's interface.
4. **A private socket (`-L xscore`).** Sessions that do not appear in `tmux ls`
   are not "managed" sessions. The default socket is used; `--socket` overrides.
5. **No xscore rows for tmux** (`data --tmux work:1`). Rejected once one control
   link serves a whole session: `%output %N` needs a `%N → PaneId → buffer` map,
   so panes must be rows.
6. **Exposing our own `-CC` surface to frontends.** Not rejected, deferred: it is
   purely additive once xscore is a working control client.

## Supersedes

- **ADR 0001** — the 3-tier hierarchy is now scoped by backend: a `Pipe` link
  serves one pane, a `Control` link serves a session.
- **ADR 0002** — `Sink` is no longer an optional per-link policy object; it is a
  per-link demultiplexer, present on every link.
- **ADR 0006** — `Window` mirrors a tmux window for tmux sessions; it is no
  longer a pure xscore-side layout container there.
- **ADR 0007** — the raw/tmux split on `Link` is gone; the workspace decides.
- **ADR 0009** — the daemon now owns control clients as well as native
  transports: it is the process that holds every live connection.

ADR 0003 (external tmux) and ADR 0008 (adoption, `@N`/`%N`) stand.
