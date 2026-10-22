# Architecture — as built (v0.6.0)

**Status: as-built for v0.6.0.** This describes the architecture the code
implements; the design rationale is [ADR 0010](../adr/0010-session-layer-and-control-mode.md),
and [§11 Delta](#11-delta-from-v051) records what changed from v0.5.1.

For the v0.5.1 design (Link × Window × Pane, `?tmux=on`):

```bash
git show v0.5.1:doc/dev/arch/arch.md
```

This file is the shape, not the argument. Rationale lives in the ADRs:

| ADR | Owns |
|---|---|
| [0001](../adr/0001-link-pane-model.md) | Link / Window / Pane as addressable tiers |
| [0002](../adr/0002-transport-sink-split.md) | Transport vs Sink split — *Sink returns as a per-link demultiplexer (§4)* |
| [0003](../adr/0003-external-tmux-server.md) | External tmux, never embedded |
| [0005](../adr/0005-add-open-close-del-verbs.md) | Verb semantics — *`open`/`close` move to `session`* |
| [0006](../adr/0006-window-concept.md) | Window tier |
| [0007 v2](../adr/0007-raw-default-tmux-opt-in.md) | Decoupled entities, buffer-per-pane, Q1/Q2 |
| [0008](../adr/0008-tmux-adoption.md) | tmux is the source of truth; `@N` / `%N` anchors |
| [0009](../adr/0009-daemon.md) | Resident daemon owns every live connection (native transports and control clients) |
| [0010](../adr/0010-session-layer-and-control-mode.md) | Session owns the backend; tmux via control mode, one link per session |

## 0. The model in one screen

A **session is a workspace with a backend**. The backend decides who keeps the
processes alive and who owns the structure:

```
Session ── workspace; 1:1 with a tmux session, or standalone
  ├ Window ── layout container
  ├ Pane   ── buffer + geometry
  └ Link   ── one connection (an endpoint)
```

| | `Backend::Native` | `Backend::Tmux { .. }` |
|---|---|---|
| keeps processes alive | the resident daemon | the tmux server |
| where it runs | this machine | this machine, or an ssh host (§6) |
| Window | xscore grouping; no layout engine | mirrors a tmux window (`@N`) |
| Pane | xscore buffer + geometry | is a tmux pane (`%N`) |
| Link | **N** links, one per connection you opened (`Pipe`) | **exactly 1** link: the control client (`Control`) |
| who feeds the pane buffer | the link's `PipeSink` | the session's control client, routing `%output %N` (in the daemon) |
| survives CLI exit | yes (daemon) | yes (tmux) |
| attach a human | n/a | `exec tmux attach` |
| multi-client | no | yes |

That table is why `Session` exists. "Does this go through tmux?" was never a
property of a *link*; it is a property of the *workspace* the link lives in.

### A session is a container, not a viewport

Both backends hold **N windows, each with N panes, each pane bound to at most
one link**. Cardinality is not what separates them — ownership is.

```
session dev                                  (Backend::Native)
├── window "edit"
│   ├── pane p1 ── link l1   shell:///bin/bash
│   └── pane p2 ── link l2   shell:///bin/vim
└── window "ops"
    ├── pane p3 ── link l3   ssh://u@host
    └── pane p4 ── link l4   serial:///dev/ttyUSB0

session work                                 (Backend::Tmux { session: "work" })
├── window @0
│   ├── pane %0 ── link l5
│   └── pane %1 ── link l6
└── window @1
    └── pane %2 ── link l7
```

```
Session 1:N Window      Window  1:N Pane       Pane 0..1 Link   (unbound = history only)
Session 1:N Link        Session 1:N Pane       Link 0..1 Pane   (serves at most one)
```

A native session is a *workspace*, not a single shell: it can hold several
windows, several panes, and links of **mixed kinds** at the same time (shell +
ssh + serial in one workspace is a normal case). A tmux-backed session gets the
same multiplicity from tmux, which additionally owns the layout.

`session new` creates initial content so the common path stays one step — one
window plus one link and one pane, mirroring what tmux does — but that is a
convenience, not a limit. Grow it with `window add`, `link add`, `pane add`.

## 1. Session

```rust
pub struct SessionInfo {
    pub id: SessionId,
    pub name: String,
    pub created_at: DateTime<Utc>,
    pub backend: SessionBackend,
}

pub enum SessionBackend {
    /// Links are byte pipes owned by the daemon; windows are xscore's own
    /// grouping. Dies when the daemon forgets it.
    Native,

    /// This session IS a tmux session. Windows and panes are read from tmux;
    /// xscore stores only the handles it adopted.
    Tmux {
        /// This machine, or an ssh host. Defined in §6.
        host: TmuxHostConfig,
        /// Which tmux server on that host. Resolved *on that host*, so a named
        /// socket is a different file on a different machine. Defined in §6.
        socket: TmuxSocket,
        /// tmux session name. Deliberately NOT derived from SessionId:
        /// tmux sessions are user-visible and user-nameable.
        session: String,
    },
}
```

`Backend::Tmux` is the 1:1 correspondence held as a **pointer, not a mirror**.
xscore does not keep a second copy of tmux's structure that could drift; when a
command touches a tmux-backed session it reads that session's **live** state
from its control client (see §7), which pushes changes as they happen.

## 2. Window

```rust
pub struct WindowInfo {
    pub id: WindowId,
    pub session_id: SessionId,
    pub name: String,
    pub created_at: DateTime<Utc>,
    pub backend: WindowBackend,
}

pub enum WindowBackend {
    /// xscore-side grouping. Panes inside are a list; there is no renderer,
    /// so there is no layout to describe.
    Native,
    /// Mirrors a tmux window. Layout is read from tmux on demand
    /// (`#{window_layout}`) and never stored.
    Tmux { handle: String },   // "@N"
}
```

**`WindowLayout` is deleted.** It was a five-variant enum (`single`, `tabs`,
`split_h`, `split_v`, `grid`) with zero consumers: tmux owns layout for tmux
windows, and nothing renders native ones. A stored layout hint nobody applies
reads like a feature. When a UI layer exists it can introduce a layout model
shaped by what that layer actually needs.

## 3. Pane

A pane is a **buffer with a viewport** — the unit users read and write.

```rust
pub struct PaneInfo {
    pub id: PaneId,
    pub session_id: SessionId,
    pub name: String,
    pub created_at: DateTime<Utc>,
    pub cols: u16, pub rows: u16,      // size
    pub x: u16,    pub y: u16,         // position inside its window
    pub window_id: Option<WindowId>,   // None = not displayed
    /// The connection this pane reads and writes through. In a tmux session
    /// this is the session's single control link (§4).
    pub link_id: Option<LinkId>,
    /// Address inside a shared link: `"%N"` for a tmux pane, `None` for a pipe
    /// link, which has one reader and therefore needs no address.
    pub link_key: Option<String>,
}
```

There is no `PaneBackend`. A pane's kind is its **link's** kind: a pane on a
`Control` link is a tmux pane, a pane on a `Pipe` link is a native one. Storing
it twice invites them to disagree ("this pane is a tmux pane but its link is a
pty"), and the link is the side that owns the fact.

The pane's **bytes are not in `PaneInfo`** — it is rewritten on every geometry
change and every save, and a 64 KiB scrollback is neither. Buffers live in the
daemon, keyed by pane:

- every pane, native or tmux, owns a `RingBuffer` (64 KiB);
- a native pane's buffer is filled by its pipe link's reader;
- a tmux pane's buffer is filled by `%output %N` arriving on the session's
  control client (which the daemon holds), routed by `link_key`;
- a detached tmux pane keeps its buffer too, and tmux additionally keeps the
  scrollback, reachable with `capture-pane`.

## 4. Link, Endpoint and Sink

A link is **one connection**. What a connection carries decides how many panes
may read it.

```rust
pub struct LinkInfo {
    pub id: LinkId,
    pub session_id: SessionId,
    pub name: String,
    pub created_at: DateTime<Utc>,
    pub endpoint: EndpointRef,
}

pub enum EndpointRef {
    /// A stream xscore owns: shell (pty), serial (device fd), ssh (channel).
    Pipe(TransportConfig),
    /// The control-mode client attached to this session's tmux server.
    /// Host, socket and tmux session name are NOT repeated here: they live on
    /// `SessionBackend::Tmux`, and a link must not be able to disagree with its
    /// session.
    Control,
}
```

### One tmux session, one link

A tmux session has **exactly one** link: its control client (`tmux -C attach`,
run through `ssh` when the host is remote). Every window, every pane and every
byte in that session comes through it:

```
Link (Control) ── bytes ──► Sink::dispatch
                             ├── %output %N        → pane %N's RingBuffer
                             ├── %window-add, %layout-change, %exit → state
                             └── %begin/%end        → the reply to a command
```

A native session has as many links as you opened connections. A tmux session has
one, because there is one server to talk to.

That is the shape the user's model always had — "a tmux link can be used by
several panes" — and control mode is what makes it literally true: the link is
the connection to the server, not the connection of a pane.

### Exclusivity depends on what the link carries

| link | panes it can serve | how a pane addresses itself |
|---|---|---|
| `Pipe` | at most **one** | it does not — it is the only reader |
| `Control` | **all** panes of its session | `link_key = "%N"` |

The earlier blanket rule "no link may be shared" was too strong. It is true of a
raw byte stream — two reader loops on one pty would steal each other's bytes —
and false of a protocol that names its recipients. tmux tells us which pane each
chunk belongs to, so sharing is **addressing**, not a race.

Consequences:

- a tmux pane cannot be moved to another link; it *is* a `%N` on that server, and
  moving it is `move-pane` inside the same session (D6);
- `link del` is refused for a `Control` link: it would orphan every pane in the
  session into a paneless buffer with no way back. Use `session kill` or
  `session forget` (D5);
- `link ls --session work` prints exactly one row for a tmux session, and
  `connected` on that row is the liveness of the whole session.

### Sink is back, with a different job

> **Not introduced (ADR 0010 D4 amendment).** The layout below is what sharing a
> link *needs*; what shipped is `TmuxControl`'s reader loop: `%output %N` is
> routed to the pane whose `link_key` is `%N`, and every other notification
> becomes a `TmuxEvent`. It has exactly one implementation, and a trait with one
> impl plus a trivial forwarder is the ceremony that got the first `Sink`
> deleted. Introduce the trait when a second link kind has to demultiplex. The
> section is kept because the reasoning about *why* sharing is addressing still
> holds.

`Sink` was retired when every link was exclusive: `capacity` / `grant` / `revoke`
had nothing left to decide. A control link is genuinely shared, so a per-link
demultiplexer is needed — but its keys are assigned by tmux, so there is still
nothing to grant:

```rust
pub trait Sink: Send + Sync {
    /// Called once per chunk read from the link.
    fn dispatch(&self, chunk: &[u8]) -> Vec<Dispatch>;
}

pub enum Dispatch {
    /// Bytes for one pane. `key: None` = "the link's only pane".
    Data { key: Option<String>, bytes: Vec<u8> },
    /// A structural change. Only a control link produces these.
    Event(TmuxEvent),
}
```

- `PipeSink` emits one `Data { key: None }` per read. It is a forwarding device,
  and that is all a byte stream can be.
- The session's control client parses the protocol: `%output` becomes `Data`,
  everything else becomes `Event`, unknown `%` lines are dropped (§7). (This
  used to be a `ControlSink`; ADR 0010 §D7 folded the sink away, and the client
  now lives in the daemon — §5.)

Two properties of the protocol make this parser safe, both measured rather than
assumed:

- **newlines are escaped** (`\012`), so one protocol line is one complete record
  and a payload can never split a line;
- **a read can still split a line**, so the sink keeps a carry-over buffer and
  processes only terminated lines. A screen redraw arrives as one long
  `%output`; do not assume a line fits in one read.

### Who owns the stream

| link | who owns the stream | `read` returns | `resize` means |
|---|---|---|---|
| `serial://` | xscore, a device fd | device bytes | nothing (no size) |
| `shell://` | xscore, a **pty** | terminal bytes | `TIOCSWINSZ` |
| `ssh://` | xscore, a TCP socket + SSH channel | channel bytes | channel `window-change` |
| `Control` | the `tmux` child process | protocol lines | `resize-pane` per pane |

Two rules follow:

1. **A shell link must allocate a pty.** With a plain pipe the child
   block-buffers stdout, `isatty` is false, and echo, job control and
   `SIGWINCH` are gone: `vim`, `sudo`, `python -i` all take their
   non-interactive path.
2. **`resize` must not lie.** It is real only where someone owns a size. A
   serial link has none, so the call must fail explicitly instead of returning
   ok and storing numbers in `PaneInfo`.

### What a link is not

- not a pane (panes read through it; a control link serves all of its session's)
- not a window
- not an owner of the session it connects to

## 5. Runtime topology

Both pictures below are *structure*, not call sequences — the point is that a
session holds many windows, many panes and many links.

### Native session

```mermaid
graph TB
    subgraph SESS["session dev — Backend::Native"]
        W1["window edit"]
        W2["window ops"]
        P1["pane p1"]
        P2["pane p2"]
        P3["pane p3"]
        L1["link l1<br/>shell:///bin/bash"]
        L2["link l2<br/>shell:///bin/vim"]
        L3["link l3<br/>ssh://u@host"]
        W1 --> P1
        W1 --> P2
        W2 --> P3
        P1 --> L1
        P2 --> L2
        P3 --> L3
    end

    subgraph D["daemon process"]
        DL["3 live Pipe transports"]
        DP["3 pane buffers + pumps"]
    end

    subgraph ST["state.json"]
        SS["session + 2 windows + 3 panes + 3 links<br/>handles only, never bytes"]
    end

    L1 --> DL
    L2 --> DL
    L3 --> DL
    P1 --> DP
    P2 --> DP
    P3 --> DP
    SESS --> SS
    DL --> EXT["shell child / shell child / ssh channel"]

    classDef sess fill:#f3e5f5,stroke:#7b1fa2
    classDef dd fill:#ede7f6,stroke:#5e35b1
    classDef st fill:#fff3e0,stroke:#f57c00
    classDef ext fill:#fafafa,stroke:#616161,stroke-dasharray: 5 5
    class W1,W2,P1,P2,P3,L1,L2,L3 sess
    class DL,DP dd
    class SS st
    class EXT ext
```

### tmux-backed session

```mermaid
graph LR
    SRV["tmux server<br/>session work<br/>structure + one pty per pane"]
    CLI["xscore CLI<br/>one process per command"]
    subgraph D["daemon process"]
        L["control slot: work-ctl<br/>Control link, one per session"]
        AL["alive flag<br/>false on %exit"]
        B["per-pane buffers %0, %1, %2<br/>one %output pump each"]
    end
    ST["registries<br/>windows @N, panes %N"]

    SRV -->|"%output %N and events"| L
    L -->|"%output %N, by address"| B
    L -->|"%exit"| AL
    CLI -->|"OpenTmux, Structure, TmuxCommand, CloseTmux"| L
    CLI -->|"Read and Write, by pane id"| B
    CLI -->|"rows"| ST
    L -->|"send-keys, split-window, list-windows"| SRV

    classDef ext fill:#fafafa,stroke:#616161,stroke-dasharray: 5 5
    classDef ctl fill:#e8f5e9,stroke:#2e7d32
    class SRV,CLI,ST ext
    class L,AL,B ctl
```

One link for the whole session, not one per pane: the link is the connection to
the **server**, and `%output %N` is what makes a single stream addressable by
three panes. Across ssh (§6) the same link is one channel; nothing else changes.

The control slot lives in the **daemon** (ADR 0009 §1): the client, its
`%output` subscriptions and every pane buffer are daemon state, so a CLI
invocation that ends does not end the stream. The CLI's `Read`/`Write` are
requests over the socket, keyed by pane id; `Structure` and `TmuxCommand` are
the two ways it ever touches the client. In the no-daemon build (tests, an
embedding caller) the same slot sits in the manager instead.

## 6. tmux over SSH

A tmux server may be reachable only from another machine. Since §4 established
that a session is fed by **one long-lived control client**, remote is one extra
segment in that client's command, not a second mechanism:

```
ssh -o BatchMode=yes <args...> <destination> tmux -L <sock> -C attach -t work
```

stdin and stdout of that `ssh` process are the control channel. Control
commands go in as lines; `%output`, `%begin/%end` and every notification come
back on the same channel. One connection per managed session, in both
directions.

`-C`, not `-CC`. `-CC` additionally disables echo by calling `tcgetattr` on the
client's terminal, which fails outright when stdin/stdout are pipes:

```
$ printf 'list-windows\n' | tmux -L probe -CC attach -t work
tcgetattr failed: Inappropriate ioctl for device
```

`-CC` is for clients that own a real terminal (iTerm2 allocates a pty). A pipe
client gets the identical protocol from `-C` — verified on 3.5a, where the same
pipeline returns `%begin … @0 … %end`.

### Why this is not the `ThroughTransport` mistake

An earlier draft rejected running tmux commands through a `Transport`, because a
raw stream carries prompts, banners and partial writes, and carving one command's
output out of it requires a parser we would have to invent and then keep
correct.

Control mode is the opposite case. The framing already exists and is part of
tmux's interface: `%begin <ts> <num> <flags>` opens a command's reply, `%end` /
`%error` closes it, and `%output`/`%layout-change`/… live in a separate
namespace. We parse a protocol tmux defines rather than guessing where output
stops. The rule that survives is narrower and still true: **do not multiplex
command control over a stream that has no framing of its own.**

### The link is a process, not a reused session

"Run `tmux -C` through the ssh link" is the right idea with one trap: it must be
a **dedicated exec**, not an existing interactive shell link. If the control
protocol rides a shell that also has a prompt, echo and job control, those bytes
land in the same stream as `%output` and the parser can no longer tell protocol
from chatter — the same failure that killed `ThroughTransport`.

So the control link's transport is always a freshly spawned process:

```
local : tmux     -L <sock> -C attach -t <session>
remote: ssh <args> <dest> tmux -L <sock> -C attach -t <session>
```

If the goal is to avoid a second TCP connection, the place to do it is OpenSSH's
`ControlMaster`, not an interactive shell.

### Two exec shapes, on purpose

| what | how | why |
|---|---|---|
| a **managed** session (we hold its control client) | one persistent `tmux -C attach` (locally, or through one `ssh`) | streams bytes and pushes events |
| **enumeration and one-shot** (`session ls` across hosts, probing, `adopt`, `kill` of a session we are not attached to) | one `tmux <cmd>` exec per call (`ssh <dest> tmux ...`) | request/response, no state to keep |

Mixing them up is the bug to avoid: data must never go through a one-shot exec
(nothing to stream from), and enumeration must never require attaching (that
would leave a client per session just to list names).

### Escaping has one layer here, not two

With a one-shot `ssh <dest> tmux send-keys ...`, `ssh` joins its argv and hands
the string to the **remote shell**, so a payload is parsed twice and large
payloads hit `ARG_MAX`. On the control channel there is no shell: the line goes
straight to tmux's own command parser, so a write is

```
send-keys -t %0 -l '<text escaped for tmux>'
```

Measured: `;`, `"`, `$` and backticks all arrive literally inside `-l` quotes.

Two limits remain and are not fixable at this layer:

- **NUL cannot be sent.** The protocol is line-oriented; a byte stream carrying
  `\0` has no representation. Anything needing it is not a tmux link.
- **tmux escaping is still real.** A payload with `'`, `\` or a newline must be
  quoted for tmux's parser, and getting it wrong is a silent mis-type rather
  than an error. This is the one place where "escape once, exactly" has to be
  written and tested rather than assumed.

### Credentials

xscore stores **no** SSH credentials. It execs the system `ssh`, so
`~/.ssh/config`, `ProxyJump`, agent forwarding and `Match` blocks all apply —
none of which the `russh`-based `ssh://` link path reads, because that client is
driven by a `TransportConfig`. Two jobs, two mechanisms:

| | how it connects | why |
|---|---|---|
| `ssh://` link | `russh`, from `TransportConfig` | xscore needs the shell's *bytes* |
| tmux session over ssh | system `ssh`, one control channel | xscore needs the user's identity and config |

`BatchMode=yes` is mandatory: a password or host-key prompt would block a
non-interactive CLI forever. Failing fast is correct — a first-time host is
trusted once, by the user, in a real terminal. `ConnectTimeout=5` bounds the
other hang. Connection reuse is not needed the way it was for per-command execs;
there is one connection per session by construction.

### Failure semantics

| what happened | signal | error | rows |
|---|---|---|---|
| session killed | `%exit` on the control channel | `TmuxTargetGone` | prune that session |
| ssh dies, times out, or auth fails | channel EOF / exit 255 on a one-shot | `TmuxUnreachable { host }` | **prune nothing** |
| one-shot tmux error | exit 1–254 | `TmuxTargetGone` | prune that row |

The second row is load-bearing: **an unreachable host must never delete rows.**
A laptop on a train would otherwise forget every remote session it manages, and
the next `ls` would report a workspace that still exists.

### Listing is local, showing is live

`session ls` never touches the network: it prints what `state.json` knows plus a
`HOST` column, so listing ten sessions across three hosts is instant and works
offline. `session show`, `attach`, `data` and window/pane mutations use the live
control client (or a one-shot exec when the session is not managed). `ls
--refresh` opts into a network pass.

### Naming

Two machines can each have `work`. The key is `(host, socket, session)`;
`SessionId` is the stable identity. The CLI reference is `name` when
unambiguous, `<host>/<name>` otherwise (`devbox/work`, `local/work`).

### Attach

```
ssh -t <dest> tmux -L <sock> attach -t work
```

exec'd in place of `xscore`. The remote TTY, resize and `Ctrl-b` belong to tmux
exactly as if the user had typed that command; xscore is out of the loop and
holds no connection.

### Testing without a network

`TmuxHost` is an argv prefix: `[]` locally, `["ssh", "-o", "BatchMode=yes", ...]`
remotely. Tests substitute the prefix, so a fake `ssh` (exits 255) and a fake
`tmux` cover the remote paths. A real control-mode transcript captured from
tmux 3.5a is the parser's fixture.

## 7. Events, not polling

There is no refresh pass and no sync command. A managed tmux session has a
control client attached, and that client pushes everything:

| notification | meaning | what xscore does |
|---|---|---|
| `%output %N <escaped>` | pane bytes | append to `%N`'s RingBuffer |
| `%window-add @N` | window appeared | insert `WindowInfo` |
| `%window-close` / `%unlinked-window-close @N` | window gone | delete it and its panes |
| `%window-renamed @N` | rename | update `name` |
| `%layout-change @N <old> <new> <flags>` | geometry | update pane sizes |
| `%session-window-changed $N @M` | current window | update a `active` flag |
| `%session-changed`, `%sessions-changed` | session set | update `SessionInfo` |
| `%exit [reason]` | client detached / session over | detach; prune if the session is gone |

Parser rules, both learned from real output rather than documentation:

- **Route by prefix, not by position.** `%begin <ts> <num> <flags>`, the
  command's output lines, then `%end` (or `%error`) form one block; events may
  arrive *inside* a block. A parser that assumes "anything between begin and end
  is output" breaks the first time a window appears while a command runs.
- **Ignore unknown `%` lines.** tmux renames notifications across versions —
  3.5a emits `%unlinked-window-close` where older releases emitted
  `%window-close`. Unknown notifications are skipped, never fatal.

**Structure is enumerated on touch, not mirrored.** The reader is a one-shot
process: it does not exist between commands, so nothing of its own is
subscribed, and an event that arrives while it is gone has nobody to notify.
`window list` / `pane list` / `session show` therefore ask for the structure
(`Structure` on the daemon, one `list-windows` plus one `list-panes` per window
on any client) and reconcile the rows from that answer. Events keep their two
real jobs: in the daemon, `%exit` is **liveness** (`alive=false`, and the
client is re-opened or the slot dropped) and dropping a closed pane's buffer
happens through the row reconciliation that follows from the enumeration; in
the in-process build, where the client lives as long as the manager, events
still update rows as they arrive. No command ever waits for an event, and
nothing polls: "on touch" is one request per listing, not a ticker.

Consequences of this being event-driven:

- no `refresh`, no `--heal`, no global pass, no geometry polling;
- `%exit` distinguishes "session gone" from "host unreachable" without exit-code
  guesswork;
- a detached pane keeps its buffer (§3) and `data read` still returns what
  arrived before the detach, but nothing new arrives until a client re-attaches;
- a pane discovered late has no history: tmux replays nothing, so its buffer
  starts empty and fills from the next byte (§13).

`capture-pane` remains available as an explicit screen query (`pane screen`),
because "the last 200 lines as rendered" and "the bytes since I last looked" are
different questions and both are legitimate. It is never what `data read`
returns.

## 8. Lifecycle and invariants

```mermaid
stateDiagram-v2
    state "Session" as SS {
        [*] --> Live: session new / adopt
        Live --> Gone: session kill
        Live --> Unmanaged: session forget
        note right of Live
            native: daemon owns it
            tmux: tmux owns it
        end note
    }

    state "Link" as LL {
        [*] --> Bound: link add, or pane born in tmux
        Bound --> Gone: link del on a native link
        Bound --> Unmanaged: link del on a tmux link
    }

    state "Pane source" as PP {
        [*] --> Unbound
        Unbound --> Bound: pane attach
        Bound --> Unbound: pane detach
    }
```

Invariants and what enforces each:

- **A `Pipe` link serves at most one pane.** Checked by looking for an existing
  pane on the link; there is no capacity beyond that.
- **A `Control` link serves every pane of its session**, each with a distinct
  `link_key` (`%N`). Uniqueness of `link_key` inside a link is the invariant that
  keeps `%output` routing unambiguous.
- **A tmux session has exactly one link.** `session new --tmux` and
  `session adopt` create it; `session kill` closes it. There is no second link
  to a server, so `link add` is rejected in a tmux session.
- **A pane's link is in the same session.** Cross-session moves only via an
  explicit `pane move --session`, never as a side effect of `attach`.
- **A tmux pane cannot be detached, only killed.** Its `link_key` is its
  identity on that server; `pane detach` refuses.
- **A tmux handle is never an index.** `@N` / `%N` only — indices shift on
  rename and reorder.
- **Binding ≠ liveness.** `pane show` reports `link_id`; `link show` reports
  `connected` (probed from the daemon for native links). A native link whose
  transport died — the daemon exited or restarted — keeps its binding and loses
  its connection → `LinkNotConnected`, never `NotAttached`.
- **Killing a session is the only cascade.** It removes its windows, panes and
  links and nothing else.
- **`link del` is refused for a `Control` link.** Every pane in the session
  routes through it, so deleting the link would leave them with buffers and no
  way back. Teardown is `session kill` (destroy) or `session forget` (stop
  managing).
- **`session forget` leaves the backend running.** Drops xscore's rows only
  (tmux-backed sessions; for native it is equivalent to `kill`, because the
  daemon is the backend).

## 9. Module structure

```
crates/xscore/src/
  core/        ids, error, address, session, link, pane, window, registries,
               manager, state, buffer (RingBuffer), events
  model/       SessionInfo, LinkInfo, WindowInfo, PaneInfo,
               SessionBackend, WindowBackend, EndpointRef, TransportConfig
  backends/    PaneBridge: DirectBridge (a pipe's pump + buffer),
               DaemonBridge (the daemon owns both), ControlBridge (a pane on
               its session's control client)
  transport/   Transport trait, shell, serial, ssh, NoopTransport, DaemonTransport
  tmux/        TmuxControl (spawn, reader/writer threads, correlation),
               protocol (pure parsing), format (the -F parsers and the command
               lines: send-keys, new-window, split-window, resize-pane)
  daemon/      protocol, server, client
  url/         URL parser
  config/      link templates
  cli/         session, window, pane, link, data, daemon, state
```

`backends/` stays — it is the per-pane conduit — but it no longer holds the
pane's buffer: that moved to `core/buffer.rs`, because it belongs to the pane,
not to the connection. There is **no `sink/`**: the demultiplexer is
`TmuxControl`'s reader loop (ADR 0010, D4 amendment), so a link kind is
distinguished by `EndpointRef` and by which bridge its panes get, not by a trait
object.

```mermaid
graph LR
    CLI["cli/"]
    CORE["core/"]
    MODEL["model/"]
    BACK["backends/"]
    TR["transport/"]
    TM["tmux/"]
    DM["daemon/"]
    URL["url/"]
    CFG["config/"]

    CLI --> CORE
    CLI --> MODEL
    CLI --> URL
    CLI --> CFG
    CLI --> DM

    CORE --> MODEL
    CORE --> BACK
    CORE --> TR
    CORE --> TM
    CORE --> DM
    CORE --> URL
    CORE --> CFG

    BACK --> TR
    BACK --> TM
    BACK --> MODEL

    TM --> TR
    TM --> MODEL

    TR --> MODEL

    DM --> TR
    DM --> CORE

    URL --> MODEL
    CFG --> MODEL

    classDef bad stroke:#c62828,stroke-width:2px,stroke-dasharray: 5 5
    CLI -.->|forbidden| TR
    CLI -.->|forbidden| TM
    CLI -.->|forbidden| BACK
    MODEL -.->|forbidden| CORE
    URL -.->|forbidden| CORE
    CFG -.->|forbidden| CORE
    class TR,TM,BACK,CORE bad
```

Two notes that survive from v0.5.1:

- `core ↔ daemon` is the one tolerated cycle: `core` holds a `DaemonClient`
  field and the client needs `Error` / ids. Tolerable while the back-edge only
  touches those leaf types; if it grows, extract a `LinkStore` trait into
  `core`.
- `daemon/server` is reachable only from `main.rs` (`daemon serve`), never from
  `core`. That keeps "run in-process" and "run as a server" from collapsing into
  one path.

## 10. Command surface

```bash
# Session — the workspace
xscore session ls [--all] [--host H] [--refresh]
xscore session new <name> [URL] [-c CONFIG] [--tmux] [--socket S]
                          # URL: shell://cmd?cwd=…&cols=…&rows=…
                          #      serial://dev?baud=…   ssh://user@host?…
                          # --tmux: the URL names the host (ssh://…), or ?cmd=…
xscore session adopt <name> [--ssh DEST] [--socket S]   # take over an existing tmux session
xscore session show <ref>                    # its windows / panes / links
xscore session attach <ref>                  # tmux: exec tmux attach; native: unsupported
xscore session kill <ref>                    # destroy the backend and the rows
xscore session forget <ref>                  # drop xscore rows, leave backend running
xscore session rename <ref> <new>

# Window — scoped to a session
xscore window list --session <s>
xscore window add  --session <s> [--name N] [--cols N --rows N]
xscore window del  --id <w>                  # panes survive as orphans

# Pane — scoped to a session
xscore pane list [--session <s>] [--window <w>] [--link <l>]
xscore pane add  --session <s> [--window <w>] [--cols N --rows N]
xscore pane del  --id <p>
xscore pane attach --pane <p> --link <l>     # pipe links only
xscore pane detach --pane <p>                # pipe links only
xscore pane move   --pane <p> --x X --y Y    # within the window
xscore pane screen <p> [--history N]         # rendered grid (not implemented)

# Link — native sessions
xscore link add <url> --session <s>
xscore link ls  [--session <s>]
xscore link del <l>

# Data — any pane
xscore data read  --pane <p> [--bytes N]
xscore data write --pane <p> [< file | --from-file F]
xscore data resize --pane <p> --cols C --rows R
```

Notes:

- `<ref>` is a session `name`, or `<host>/<name>` when that name exists on more
  than one host (`devbox/work`); `local/<name>` is always accepted. Two machines
  can each have `work`, so names are not globally unique — see §6.
- Every `<ref>`-taking verb resolves the host first and then contacts only that
  host. No command ever fans out across hosts.
- `--cmd` / `--cwd` / `--cols` / `--rows` are **URL query parameters**, not
  flags: the first link and the pane that shows it are described by one string
  (`shell:///bin/bash?cwd=/var/log&cols=120&rows=40`). With `--tmux` the same
  string names the server (`shell://` for the local one, `ssh://[user@]host`
  for a remote one), so `session new` keeps only `--tmux` and `--socket`.
  Unknown keys are refused rather than ignored. The chosen scheme is stored on
  the control link's address (`Shell` vs `Ssh`), which is what lets `link list`
  say `control/shell` or `control/ssh` instead of flattening both to `shell`.
- `session new` starts the workspace with **one window, one link, one pane**
  so the common path stays a single command; there is no empty-workspace form
  (`--empty` is gone) and a URL is optional (`$SHELL` at 80x24).
- `session new` creates a *workspace*, not one viewport. A native session can
  hold several windows and links of mixed kinds (shell + ssh + serial) at the
  same time — see §0.
- For a tmux session, `pane add` is atomic: `split-window`, read `%N`, insert a
  pane row carrying `link_key = %N` on the session's existing link. No link is
  created — a tmux session has exactly one (§4).
- `link add` is rejected inside a tmux session for the same reason; `link ls
  --session work` shows exactly one row.
- `session attach` replaces `link open`; `session kill` / `session forget`
  replace `link close`. `link` loses both verbs.
- `?tmux=on|auto|local|remote` is **removed**. Whether a session is tmux-backed
  is decided at `session new` / `session adopt`.
- `tmux://` is **not** a link URL. Adoption happens at the session level.

## 11. Delta from v0.5.1

**Status: landed.** `v3` is unreleased, so this section describes the change as
it exists in the code, not as an intention. Two rows below were revised while
implementing it: `Sink` as a trait was *not* reintroduced (ADR 0010, D4
amendment), and `backends/` did *not* disappear — only its buffer moved.

### Removed

| Gone | Why |
|---|---|
| `Sink::capacity` / `grant` / `revoke`, `RawSink`, `TmuxSink: Sink`, the `sink/` module | tmux assigns the keys (`%N`) itself, so there is nothing to grant. The `Sink` trait was **not** reintroduced: one implementation is not a trait (§4) |
| `Link.sink: Box<dyn Sink>` | it was dead: one `no-op attach` call, then never touched |
| `Link.active_pane` (Q1 ledger) | a `Pipe` link has one reader by construction; a `Control` link addresses panes by key |
| `SinkConfig::{Raw, Tmux}` | replaced by `SessionBackend`; a link no longer knows about tmux |
| `?tmux=on\|auto\|local\|remote` | tmux-ness is a session property |
| `socket: Option<String>` | replaced by `TmuxSocket { Default, Named, Path }`: a named socket resolves on the *host*, so a name is not a location |
| `WindowInfo.tmux_window_handle`, `WindowInfo.layout`, `PaneInfo.sink_handle` | folded into `WindowBackend::Tmux { handle }` (`@N`) and the pane's `link_key` (`%N`) |
| `WindowLayout` (5 variants) | no consumer; tmux owns layout, native has no renderer |
| `link open` / `link close` | `session attach` / `session kill` / `session forget` |
| `link sync [--heal]` (`SyncReport`) | replaced by control-mode events (§7): no mode flag, no global pass, no polling |
| `TmuxBridge` (capture-pane per operation) | `ControlBridge`: `%output` fills the pane's buffer, `send-keys` is one line |
| `backends/ring_buffer.rs` | the pane's buffer is `core/buffer.rs`; `backends/` keeps the conduit |

### Added

| New | Role |
|---|---|
| `Session` + `SessionId` + `SessionBackend` | the workspace, and who keeps it alive |
| `WindowBackend::{Native, Tmux}` | who owns a window's structure |
| `EndpointRef::{Pipe(TransportConfig), Control}` | a stream xscore owns, or the session's one control connection |
| `ControlBridge` | a pane's view of its session's control client: buffer reads, `send-keys` writes |
| `tmux::TmuxHost` (`Local`, `Ssh`) | argv-prefix runner: local `tmux`, or `ssh <dest> tmux` once per command (§6) |
| `session adopt` / `session forget` | manage sessions xscore did not create; stop managing without destroying |
| `pane add --window` on a tmux session | splits the tmux window and inserts the pane row with `link_key = %N` on the session's one link |

### Changed

| Area | v0.5.1 | v0.6.0 |
|---|---|---|
| state schema | v2: flat `links`/`windows`/`panes` | **v3**: adds `sessions`; every child carries `session_id`; backends replace `sink` / `sink_handle` / `tmux_window_handle` |
| `link add` | no session | requires `--session` (a native session) |
| Q1 | special case in `attach_pane` | per link kind: a `Pipe` link one pane, a `Control` link every pane in its session |
| tmux control | `Sink` methods + `link sync` + one exec per call | one control client per session; notifications update state (§7) |
| remote tmux | `?tmux=remote`, never read — always local | `SessionBackend::Tmux { host: Ssh { .. } }`; one `ssh` exec per tmux command |
| `Window.layout` | stored enum | gone; tmux owns layout, and `WindowBackend` says who owns the window |
| `Link.transport` + `Link.sink` | two fields, one of them dead | `LinkInfo.endpoint`: `Pipe(TransportConfig)` or `Control` (host/socket/session come from the session) |

### Behaviour that must change (defects, not features)

These are wrong in v0.5.1 and are not optional for v0.6.0:

| today | why it matters | v0.6.0 |
|---|---|---|
| `ShellTransport` uses `Stdio::piped()`, not a pty | child block-buffers stdout; no `isatty`, echo, job control or `SIGWINCH`; TUIs and `sudo` misbehave | allocate a pty, set `TERM` (§4) |
| `portable-pty` is a declared dependency with **zero uses** | dead weight; the pty path was intended and never finished | used by the shell side of `PipeChannel` |
| `data resize` on a direct pane returns ok and does nothing | a silent lie: the caller sees success | `TIOCSWINSZ` / `window-change` / `resize-pane`, else an explicit error (§4) |
| `ShellTransport` holds **one** mutex across `read` and `write` | a blocking read starves writes; the 10 ms poll and `spawn_blocking` are workarounds, and `data write` can stall behind a queued read | split the pty into independent read and write halves |

### Migration (v2 → v3)

`schema_version: 3`; v2 files are rejected with an actionable error, not
silently converted (same policy as v0.4.0). The one-time migration:

1. each existing tmux link → `Session { backend: Tmux { socket, session } }`
   named after the link; its `@N` / `%N` handles move into the child backends.
2. each existing native (raw) link → `Session { backend: Native }` named after
   the link; its pane keeps the binding.
3. windows and panes are re-parented to the session they came from.
4. `link sync` output is discarded — it described cached structure that is now
   event-driven.

A v3 file written by the pre-PR4 build (v3 was never released) is upgraded by
the same command, in a second step: every link still carrying the old tmux sink
turns its session into `SessionBackend::Tmux { host: Local, socket:
Named(<XSCORE_SOCKET or "xscore">), session: <the sink's name or
xscore-<link id>> }`, and that session's link, window and pane rows are
**dropped** — the control link is created when the session is next attached, and
tmux re-enumerates the structure. Native sessions keep their rows; every row is
rewritten in the field shape above. Loading such a file without migrating is a
hard error naming the command.

## 12. Open decisions

| # | Question | Lean |
|---|---|---|
| D1 | Does a tmux-backed session have explicit `Link`s, and if so how many? | **Decided: one per session.** The link is the control connection to the server; panes address themselves with `link_key = %N`. This is the original "a tmux link may be used by several panes" model, made literal by control mode. |
| D2 | Window mirroring for tmux sessions: mirror `@N`, or keep one tmux window and treat xscore windows as private? | **Decided: mirror `@N`.** `tmux attach` must show what xscore shows; a private grouping would be a second, contradicting structure. |
| D3 | `WindowLayout`: delete for good, or keep a hint for native windows? | **Delete.** No consumer; reintroduce with the renderer. |
| D4 | tmux socket: private `-L xscore`, or the user's default socket? | **Decided:** the default socket; `--socket` overrides. Sessions invisible in `tmux ls` are not "managed". Tests pass `--socket` explicitly. |
| D5 | `link del` on a tmux-backed link | **Refused.** The control link carries every pane in the session. Teardown is `session kill` (destroy) or `session forget` (stop managing). |
| D6 | Can a pane move between sessions? | **No implicit moves.** `pane move --session` is explicit and native-only. |
| D7 | `session forget` — needed, or is `kill` enough? | **Keep it.** Mirrors v0.5.1's `del` vs `close`: destroy vs stop managing. |
| D8 | Alternative to mirroring: address tmux directly (`data --tmux work:1`) and have no xscore rows for tmux at all | **Rejected.** Once one control link serves every pane, panes must be rows: `%output %N` needs a `%N → PaneId → RingBuffer` mapping. Without rows there is nowhere to route a stream. |
| D9 | Does xscore pass `ControlMaster=auto` by default, or only when the user asks? | **Pass it**, overridable via `--ssh-arg`. Without it a burst of commands pays a full handshake each time; with it xscore writes no pooling code. |
| D10 | Should `ssh://` links move to the system `ssh` too, leaving one SSH mechanism? | **Not in this change.** `russh` gives a byte stream and in-process control; a link is not an exec client. Revisit only if storing password auth becomes a liability. |
| D11 | Is control mode the only tmux integration, or do one-shot execs stay? | **Both, split by job** (§6): a control client for a managed session, one exec for enumeration. |
| D12 | `-C` or `-CC`? | **Decided: `-C`.** `-CC` calls `tcgetattr` to disable echo and dies with a pipe (`tcgetattr failed: Inappropriate ioctl for device`, measured on 3.5a); it is for clients that own a terminal. `-C` gives the same protocol to a pipe client. |
| D13 | Is "-CC compatibility" consuming tmux's control protocol, or exposing one of our own? | **Decided: consuming.** xscore is the control client. An `xscore control` surface for GUI frontends is additive and can land later without changing this model. |

## 13. What this does not solve

- **tmux output is line-framed.** `%output` is escaped text on a line protocol,
  so `\0` cannot be sent and arbitrary binary is unsafe. It is terminal bytes,
  not any bytes.
- **The control client is a single point of failure per session.** If it dies,
  every pane in that session stops streaming at once (tmux keeps running;
  re-attaching resumes, but the gap is lost). It now lives in the daemon, so
  "the client died" and "the daemon died" are the same event.
- **No replay.** Attaching a control client does not re-send earlier output;
  history exists only as scrollback (`capture-pane -S -`).
- **Pane buffers live in daemon memory and are lost on `daemon stop`.** They are
  the same kind of object as a native pane's buffer: seconds-to-minutes of
  scrollback, not a durability contract. The tmux session is untouched; a read
  after a restart returns nothing. A pane discovered late has no history either
  — tmux replays nothing — so its buffer starts at the next byte.
- **No exit status from a pane.** Liveness arrives as a pane-dead flag, not as a
  `waitpid` status.
- **No renderer.** Native windows have no layout model and no drawing code;
  `pane.cols/rows/x/y` are recorded but unenforced. The tmux backend is the only
  one that can display anything to a human today.
- **Remote tmux is designed, not implemented.** §6 is the contract; local tmux
  ships first, `TmuxHostConfig::Ssh` follows without changing the model.
- **No remote *native* sessions.** A `Backend::Native` session lives on this
  machine, because its links are owned by the local daemon. "A shell on another
  host" is an `ssh://` link inside a local session, not a remote session.
- **Concurrent `state.json` writers still last-writer-wins.** Unchanged from
  v0.5.1. Sessions shrink the blast radius (mutations are scoped) but add no
  locking.
