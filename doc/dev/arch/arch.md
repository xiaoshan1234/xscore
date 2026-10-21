# Architecture — target design

**Status: design only.** This describes the architecture for **v0.6.0**. The
code at `HEAD` is v0.5.1 and does not match it yet — see
[§11 Delta](#11-delta-from-v051) for the gap and the migration.

For the as-built description of v0.5.1:

```bash
git show v0.5.1:doc/dev/arch/arch.md
```

This file is the shape, not the argument. Rationale lives in the ADRs:

| ADR | Owns |
|---|---|
| [0001](../adr/0001-link-pane-model.md) | Link / Window / Pane as addressable tiers |
| [0002](../adr/0002-transport-sink-split.md) | Transport vs Sink split — *Sink is retired by this design* |
| [0003](../adr/0003-external-tmux-server.md) | External tmux, never embedded |
| [0005](../adr/0005-add-open-close-del-verbs.md) | Verb semantics — *`open`/`close` move to `session`* |
| [0006](../adr/0006-window-concept.md) | Window tier |
| [0007 v2](../adr/0007-raw-default-tmux-opt-in.md) | Decoupled entities, buffer-per-pane, Q1/Q2 |
| [0008](../adr/0008-tmux-adoption.md) | tmux is the source of truth; `@N` / `%N` anchors |
| [0009](../adr/0009-daemon.md) | Resident daemon owns native transports and buffers |

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
| Link endpoint | `Pipe` — a byte stream | `Screen` — capture / send-keys |
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
convenience, not a limit. Grow it with `window new`, `link add`, `pane new`.

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
command touches a tmux-backed session it **refreshes that one session** from
tmux (see §7) and acts on what tmux reports.

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
    pub link_id: Option<LinkId>,       // None = no source; history only
    pub backend: PaneBackend,
}

pub enum PaneBackend {
    Native,
    /// This pane IS tmux pane `%N`. Its identity is the handle: it cannot be
    /// re-bound to another link, and it dies with its tmux session.
    Tmux { handle: String },
}
```

The pane's **bytes are not in `PaneInfo`** — it is rewritten on every geometry
change and every save, and a 64 KiB scrollback is neither. The bytes live beside
the channel:

- native pane → `RingBuffer` (64 KiB) in the daemon (or in-process in tests).
  Survives `detach`; read is a non-destructive snapshot.
- tmux pane → no local buffer. tmux *is* the scrollback; each read is a
  `capture-pane`.

## 4. Link and Endpoint

A link is **one connection**: a handle you can list and hand to at most one pane.

```rust
pub struct LinkInfo {
    pub id: LinkId,
    pub session_id: SessionId,
    pub name: String,
    pub created_at: DateTime<Utc>,
    /// What this link is attached to. The tmux session is NOT repeated here:
    /// it is implied by `session_id` (`SessionInfo::backend`). Only the pane
    /// handle is local to the link, so a session rename can never leave a
    /// stale name behind in a link.
    pub endpoint: EndpointRef,
}

pub enum EndpointRef {
    /// shell / serial / ssh — a byte stream, owned by the daemon.
    Pipe(TransportConfig),
    /// A tmux pane in this session's tmux session.
    Screen { pane: String },   // "%N"
}

// Runtime, never serialized:
pub enum Endpoint {
    Pipe(Arc<dyn Transport>),
    Screen { tmux: Arc<TmuxSession>, pane: String },
}
```

`Endpoint` is the honest split. A tmux pane cannot pretend to be a byte stream:
reads are non-destructive screen snapshots, writes are key injection, and
binary output is not recoverable. Stating that once here is better than
discovering it per call site.

### Every link is exclusive

Q1 ("a direct link serves one pane at a time") stops being a special case and
becomes the only rule: **a link is handed to at most one pane**. Multi-pane is a
*backend* capability — `Backend::Tmux` gives you N panes by giving you N links,
one per tmux pane.

That is also why `Sink` is retired (§10): `capacity` / `grant` / `revoke` only
existed to express "this link may be shared", and after this change nothing may.

### Who owns the stream

`Pipe` is not one mechanism. Three families sit behind it, and they do not
behave alike:

| link | who owns the stream | what `read` returns | `resize` means |
|---|---|---|---|
| `serial://` | xscore, a device fd | raw device bytes | nothing (no size) |
| `shell://` | xscore, a **pty** | terminal output, escapes included | `TIOCSWINSZ` |
| `ssh://` | xscore, a TCP socket + SSH channel | channel bytes | channel `window-change` |
| `tmux://` (any host) | **tmux** | a rendered screen (`Screen`, not `Pipe`) | `resize-pane` |

Two rules follow, and they belong in the design rather than in a bug report:

1. **A shell link must allocate a pty.** With a plain pipe the child
   block-buffers stdout (output appears only at 4 KiB or exit), `isatty` is
   false, echo and job control are gone, and `vim`, `sudo`, `python -i` and every
   TUI take their non-interactive path. A "shell session manager" that cannot
   run an interactive program is not one.
2. **`resize` must not lie.** It is real only where someone owns the size:
   `TIOCSWINSZ` for a pty, `window-change` for SSH, `resize-pane` for tmux. For a
   serial link there is no size, so the call must fail explicitly instead of
   returning ok and storing the numbers in `PaneInfo`.

### What a link is not

- not a tmux session (a session is a session)
- not a pane (a pane consumes a link)
- not an owner of windows or panes

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
graph TB
    subgraph SESS["session work — Backend::Tmux"]
        W0["window @0"]
        W1["window @1"]
        Q0["pane %0"]
        Q1["pane %1"]
        Q2["pane %2"]
        M0["link m0<br/>Screen %0"]
        M1["link m1<br/>Screen %1"]
        M2["link m2<br/>Screen %2"]
        W0 --> Q0
        W0 --> Q1
        W1 --> Q2
        Q0 --> M0
        Q1 --> M1
        Q2 --> M2
    end

    subgraph ST["state.json"]
        TS["session + 2 windows + 3 panes + 3 links<br/>handles: @N and %N only"]
    end

    H["ssh client → sshd → login shell<br/>present only when host is Ssh"]
    C["tmux client<br/>one exec per command"]
    TMS["tmux server on that host<br/>session work: windows, panes, layout<br/>THE source of truth"]

    SESS --> ST
    M0 --> C
    M1 --> C
    M2 --> C
    H -.-> C
    C -->|"unix socket: -L name, -S path, or default"| TMS

    classDef sess fill:#f3e5f5,stroke:#7b1fa2
    classDef st fill:#fff3e0,stroke:#f57c00
    classDef hop fill:#e8f5e9,stroke:#2e7d32
    classDef ext fill:#fafafa,stroke:#616161,stroke-dasharray: 5 5
    class W0,W1,Q0,Q1,Q2,M0,M1,M2 sess
    class TS st
    class H,C hop
    class TMS ext
```

The difference between the two pictures is the whole design: **native keeps
bytes, tmux keeps structure.** xscore keeps handles in both cases, and bytes
only in the native case. The tmux picture holds whether its server is on this
machine or across ssh: the hop changes the path, not the model (§6).

## 6. tmux over SSH

A tmux server may be reachable only from another machine — that is where the
sessions people most want xscore to manage usually live. So the host is part of
the backend, not an afterthought:

```rust
pub enum SessionBackend {
    Native,
    Tmux {
        host: TmuxHostConfig,
        socket: TmuxSocket,
        session: String,
    },
}

pub enum TmuxHostConfig {
    Local,
    /// `user@box`, plus the ssh flags that are part of "which server is this"
    /// (`-p 2222`, `-i ~/.ssh/work`). Persisted; never a secret.
    Ssh { destination: String, args: Vec<String> },
}

pub enum TmuxSocket {
    /// No flag: whatever the host calls its default socket.
    Default,
    /// `-L <name>`: resolved on the host, under its own `TMUX_TMPDIR`.
    Named(String),
    /// `-S <path>`: absolute on the host. Used when the socket had to be
    /// resolved rather than guessed — see "Sockets are per host" below.
    Path(String),
}
```

### Same protocol, different path

A local tmux server and a remote one speak the same commands, hold the same
`@N` / `%N` handles, and are refreshed the same way. What differs is the path to
them:

| | `TmuxHostConfig::Local` | `TmuxHostConfig::Ssh` |
|---|---|---|
| process chain | `xscore → tmux` | `xscore → ssh → sshd → login shell → tmux` |
| who authenticates | nobody | the user's ssh identity and config |
| cost per command | fork + exec, ~1 ms | + one round trip (5 ms shared, 50–300 ms cold) |
| socket namespace | this machine's | **that machine's** — `-L work` is a different file there |
| host is down | tmux starts a server, or fails fast | `TmuxUnreachable`; no rows pruned |
| attach | `tmux attach -t work` | `ssh -t <dest> tmux attach -t work` |
| tmux version | known | whatever the remote runs |

Everything else is identical: the same command set, the same handle formats, the
same capture/paste path, the same refresh. So the rule is:

> **Nothing above `TmuxHost` may branch on local vs ssh.** If a caller needs to
> know which one it has, the abstraction has leaked.

### Sockets are per host

`-L work` resolves to `$TMUX_TMPDIR` (usually `/tmp`) `/tmux-<uid>/work` **on
the host that runs the command**. `TmuxSocket::Named("work")` is therefore not a
location but a name to be resolved on the target: local `-L work` and remote
`-L work` are two unrelated servers.

`TmuxSocket::Path` exists for the one case where "the default" would be silently
wrong: when xscore is itself launched from inside tmux, `$TMUX` is set and a bare
`tmux` targets *that* server rather than the default one. Inheriting the variable
would make `session new` depend on where the user happened to run it, so the
socket is resolved once at session creation and stored as a path. A row never
depends on the environment of the process that later reads it.

### Control is request/response, not a byte stream

An earlier draft called this seam `TmuxHost::ThroughTransport`: run `tmux`
commands through an existing `Transport`. **That is wrong and is dropped.** A
`Transport` is a bidirectional byte stream that may also carry login banners,
shell prompts, escape sequences and partial writes. Recovering "the output of
this one command" from it needs delimiters and a parser, and that parser breaks
on the first remote that prints something unexpected.

tmux control is one command in, one exit code and some bytes out. So it is one
`ssh` invocation per command:

```
ssh -o BatchMode=yes <args...> <destination> tmux -L <sock> list-windows -t work -F '#{window_id}\t#{window_active}'
```

What that buys:

- **the exit code is tmux's**, so "no such session" is distinguishable from
  "the network died" (see the failure table below);
- **stdout is only the command's stdout**, not a session's screen;
- **nothing to invent** — no framing, no demultiplexing, no keepalive.

Two operational details that are not optional:

- **`-o BatchMode=yes` always.** Without it a password prompt or an unknown
  host key blocks a non-interactive CLI forever. Failing fast is correct: a
  first-time host must be trusted once, by the user, in a real terminal.
  `-o ConnectTimeout=5` bounds the other hang.
- **do not assume stdout is pure.** A `Banner`/`PrintMotd` misconfiguration, a
  `ForceCommand`, or a startup file that echoes can put extra text on the wire.
  Parse only lines that match the `-F` format and drop everything else — a line
  that does not look like `@N` / `%N` is never guessed at.

### Connection reuse, not a connection pool

One `ssh` per command costs a TCP handshake and an auth round trip. The fix is
the one OpenSSH already has, not a pool we write:

```
-o ControlMaster=auto -o ControlPath=~/.ssh/cm-%r@%h:%p -o ControlPersist=60
```

Users who do remote work usually have this on already; xscore passes it as
defaults (overridable) so a burst of `pane new` calls rides one connection. Cold
round trips are 50–300 ms, shared ones ~5 ms — which is why §7 forbids any
background polling of remote sessions.

### Credentials

xscore stores **no SSH credentials** for tmux. It execs the system `ssh`, so
`~/.ssh/config`, `ProxyJump`, agent forwarding, `Include` and `Match` blocks all
apply — none of which the `russh`-based `ssh://` link path reads, because that
client is driven by a `TransportConfig`, not by the user's ssh config. Two
different jobs, two different mechanisms:

| | how it connects | why |
|---|---|---|
| `ssh://` link (`Pipe`) | `russh`, password auth from `TransportConfig` | xscore needs the *bytes* of a shell, not an exec channel |
| tmux over ssh (`Screen`) | system `ssh`, one exec per command | xscore needs the user's ssh identity and config, and never the stream |

Anything interactive fails instead of prompting. Storing a password would put a
secret in `state.json`; that is not on the table.

### Reading and writing a remote pane

`capture-pane -p` is fine over ssh: stdout is the screen text.

Writing is where naive `send-keys` breaks. `ssh` joins its argv with spaces and
hands the result to the **remote shell**, so a payload containing quotes,
`$`, backticks or newlines is parsed twice, and large payloads hit `ARG_MAX`.
So a write never puts the payload on a command line:

```
printf '%s' "$payload" | ssh <dest> 'tmux -L S load-buffer -b xscore - \
                                  && tmux -L S paste-buffer -b xscore -d -t %3'
```

The remote command line is constant; the bytes arrive on stdin. The **same path
is used for local panes**, so there is one write implementation rather than two
with different quoting rules.

### Failure semantics

| what happened | exit / signal | error | what refresh does |
|---|---|---|---|
| tmux says the target is gone | remote 1–254 | `TmuxTargetGone` | prune that row |
| ssh cannot reach or authenticate | **255** (ssh's own) | `TmuxUnreachable { host }` | **prune nothing** |
| ssh hangs | `ConnectTimeout` / call timeout | `TmuxUnreachable { host }` | **prune nothing** |

The third column is load-bearing: **a refresh that cannot reach the host must
never delete rows.** Otherwise a laptop on a train silently forgets every remote
session it manages, and the next `ls` reports a workspace that still exists.

### Listing is local, showing is live

`session ls` never touches the network: it prints what `state.json` knows plus a
`HOST` column, so listing ten sessions across three hosts is instant and works
offline. Only `session show`, `session attach`, `data`, and window/pane
mutations contact the host. `ls --refresh` opts into a network pass explicitly.

### Naming: session names are not globally unique

Two machines can each have `work`. The key is `(host, socket, session)`;
`SessionId` is the stable identity. The CLI reference is `name` when
unambiguous, `<host>/<name>` otherwise:

```
xscore session show work              # ok if only one host has "work"
xscore session show devbox/work       # disambiguated
xscore session attach devbox/work     # ssh -t devbox tmux attach -t work
```

`local/<name>` is accepted for the local host, so a script can always be
explicit.

### Attach

```
ssh -t <dest> tmux -L <sock> attach -t <session>
```

exec'd in place of `xscore`. The remote TTY, resize and `Ctrl-b` then belong to
tmux exactly as if the user had typed that command; xscore is out of the loop
and holds no connection.

### Testing without a network

`TmuxHost` is built from an **argv prefix**: `[]` locally,
`["ssh", "-o", "BatchMode=yes", ...]` remotely. Tests therefore exercise the
same code path by substituting the prefix — a script that `exec`s locally, a
`tmux` shim on `PATH`, and a fake `ssh` that fails with 255 to test
`TmuxUnreachable`. No test needs a real remote.

## 7. Refresh, not sync

A tmux-backed session has rows in `state.json` (windows, panes, links) but tmux
owns their truth. The rule, applied **per session** and only when a session is
touched:

```
touch(session) →
  tmux list-windows + list-panes -t <tmux session>
  for each adopted row whose handle is gone   → remove the row; its pane's
                                                 link_id becomes None
  for each adopted row whose geometry changed → update cols/rows/x/y
  panes that tmux reports but xscore never adopted → REPORTED, not adopted
```

Notes that make this different from v0.5.1's `link sync --heal`:

- **Scoped to one session.** A refresh can never touch another session's rows
  (v0.4.0 shipped that bug once and fixed it).
- **No adoption by default.** Panes the user created by hand in tmux stay
  invisible to xscore until `xscore session adopt <name>`.
- **No `--heal` flag.** Healing is the only behaviour; there is no mode where a
  known-stale row is kept.
- **No global pass.** Nothing walks all sessions; a session is refreshed because
  a command mentioned it.
- **Refresh is in-memory.** Removed rows and changed geometry are applied to the
  in-memory registries only; persistence rides on the normal save at the end of
  a *mutating* command. A read-only command (`ls`, `show`, `data read`) must not
  write `state.json` — otherwise every read would contend for the file and every
  user would see a mutation they did not ask for.

`session adopt <name>` is the explicit "take the current tmux structure as
xscore rows" operation — the way to start managing a session you created outside
xscore.

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

- **A link serves at most one pane.** Per-session scan on attach; no other
  capacity exists.
- **A pane's link is in the same session.** Cross-session moves only via an
  explicit `pane move --session`, never as a side effect of `attach`.
- **A tmux pane cannot be detached.** `PaneBackend::Tmux` is its identity;
  `pane detach` refuses. Destroying it is `pane kill`.
- **A tmux handle is never an index.** `@N` / `%N` only — indices shift on
  rename and reorder.
- **Binding ≠ liveness.** `pane show` reports `link_id`; `link show` reports
  `connected` (probed from the daemon for native links). A native link whose
  transport died — the daemon exited or restarted — keeps its binding and loses
  its connection → `LinkNotConnected`, never `NotAttached`.
- **Killing a session is the only cascade.** It removes its windows, panes and
  links and nothing else.
- **`link del` on a tmux link forgets the handle.** The tmux pane and its
  session survive: a handle is not an owner. To destroy tmux state use
  `pane kill` / `session kill`.
- **`session forget` leaves the backend running.** Drops xscore's rows only
  (tmux-backed sessions; for native it is equivalent to `kill`, because the
  daemon is the backend).

## 9. Module structure (target)

```
crates/xscore/src/
  core/        ids, error, address, session, link, pane, window, registries,
               manager, state, buffer (RingBuffer), refresh
  model/       SessionInfo, LinkInfo, WindowInfo, PaneInfo,
               SessionBackend, WindowBackend, PaneBackend, TransportConfig
  channel/     Channel trait, PipeChannel, ScreenChannel, DaemonChannel
  transport/   Transport trait, shell, serial, ssh, NoopTransport, DaemonTransport
  tmux/        TmuxHost (Local / Ssh), structure queries, session runtime
  daemon/      protocol, server, client
  url/         URL parser
  config/      link templates
  cli/         session, window, pane, link, data, daemon
```

`backends/` disappears: it held the conduit (`PaneBridge`) and the pane's buffer.
The conduit becomes `channel/`; the buffer moves to `core/buffer.rs` because it
belongs to the pane, not to the connection.

```mermaid
graph LR
    CLI["cli/"]
    CORE["core/"]
    MODEL["model/"]
    CHAN["channel/"]
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
    CORE --> CHAN
    CORE --> TR
    CORE --> TM
    CORE --> DM
    CORE --> URL
    CORE --> CFG

    CHAN --> TR
    CHAN --> TM
    CHAN --> MODEL

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
    CLI -.->|forbidden| CHAN
    MODEL -.->|forbidden| CORE
    URL -.->|forbidden| CORE
    CFG -.->|forbidden| CORE
    class TR,TM,CHAN,CORE bad
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
xscore session new <name> [--tmux [--ssh DEST] [--socket S]] [--empty]
                          [--cmd C] [--cwd D] [--cols N --rows N]
xscore session adopt <name> [--ssh DEST] [--socket S]   # take over an existing tmux session
xscore session show <ref>                    # its windows / panes / links
xscore session attach <ref>                  # tmux: exec tmux attach; native: unsupported
xscore session kill <ref>                    # destroy the backend and the rows
xscore session forget <ref>                  # drop xscore rows, leave backend running
xscore session rename <ref> <new>

# Window — scoped to a session
xscore window ls   --session <s>
xscore window new  --session <s> [--name N] [--cmd C] [--cwd D]
xscore window kill <w>
xscore window rename <w> <new>

# Pane — scoped to a session
xscore pane ls   [--session <s>] [--window <w>]
xscore pane new  --session <s> [--window <w>] [--cmd C] [--cwd D] [--cols N --rows N]
xscore pane kill <p>
xscore pane attach --pane <p> --link <l>     # native only
xscore pane detach --pane <p>                # native only
xscore pane move   --pane <p> --window <w>   # within the session

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
- `--cmd` / `--cwd` belong to the verbs that **start a process** (`session new`,
  `window new`, `pane new`). URLs only say how to connect. This is where the
  command that v0.5.1 silently dropped (`?tmux=on` ignored the URL's command)
  now lives.
- `session new` starts the workspace with **one window, one link, one pane**
  so the common path stays a single command. `--empty` skips that for a native
  session (bare workspace); it is a no-op for `--tmux`, where tmux always
  creates at least one pane.
- `session new` creates a *workspace*, not one viewport. A native session can
  hold several windows and links of mixed kinds (shell + ssh + serial) at the
  same time — see §0.
- For a tmux session, `pane new` is atomic: `split-window`, adopt `%N`, create
  the link, bind them. A tmux pane is born bound.
- `session attach` replaces `link open`; `session kill` / `session forget`
  replace `link close`. `link` loses both verbs.
- `?tmux=on|auto|local|remote` is **removed**. Whether a session is tmux-backed
  is decided at `session new` / `session adopt`.
- `tmux://` is **not** a link URL. Adoption happens at the session level.

## 11. Delta from v0.5.1

### Removed

| Gone | Why |
|---|---|
| `Sink` trait, `RawSink`, `TmuxSink: Sink` | no link may be shared, so `capacity` / `grant` / `revoke` have nothing to express |
| `Link.sink: Box<dyn Sink>` | it was dead: one `no-op attach` call, then never touched |
| `Link.active_pane` (Q1 ledger) | exclusivity follows from "one link, one pane" |
| `SinkConfig::{Raw, Tmux}` | replaced by `SessionBackend`; a link no longer knows about tmux |
| `?tmux=on\|auto\|local\|remote` | tmux-ness is a session property |
| `socket: Option<String>` | replaced by `TmuxSocket { Default, Named, Path }`: a named socket resolves on the *host*, so a name is not a location |
| `WindowInfo.tmux_window_handle`, `PaneInfo.sink_handle` | folded into `WindowBackend::Tmux` / `PaneBackend::Tmux` |
| `WindowLayout` (5 variants) | no consumer; tmux owns layout, native has no renderer |
| `link open` / `link close` | `session attach` / `session kill` / `session forget` |
| `link sync [--heal]` | replaced by per-session refresh (§7): same data, no mode flag, no global pass |
| `backends/` module | split into `channel/` (the conduit) and `core/buffer.rs` |

### Added

| New | Role |
|---|---|
| `Session` + `SessionId` + `SessionBackend` | the workspace, and who keeps it alive |
| `Endpoint::{Pipe, Screen}` | the honest split between byte streams and tmux screens |
| `channel/{Pipe,Screen,Daemon}Channel` | what a pane reads and writes through |
| `tmux::TmuxHost` (`Local`, `Ssh`) | argv-prefix runner: local `tmux`, or `ssh <dest> tmux` once per command (§6) |
| `session adopt` / `session forget` | manage sessions xscore did not create; stop managing without destroying |
| `pane new --window` on a tmux session | creates tmux pane + link + pane atomically |

### Changed

| Area | v0.5.1 | v0.6.0 |
|---|---|---|
| state schema | v2: flat `links`/`windows`/`panes` | **v3**: adds `sessions`; every child carries `session_id`; backends replace `sink` / `sink_handle` / `tmux_window_handle` |
| `link add` | no session | requires `--session` (a native session) |
| Q1 | special case in `attach_pane` | universal: one link → one pane |
| tmux control | `Sink` methods + `link sync` | `session` / `window` / `pane` verbs; tmux queried per session |
| remote tmux | `?tmux=remote`, never read — always local | `SessionBackend::Tmux { host: Ssh { .. } }`; one `ssh` exec per tmux command |
| `Window.layout` | stored enum | gone; `#{window_layout}` read from tmux when needed |

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
   refreshed per query.

## 12. Open decisions

| # | Question | Lean |
|---|---|---|
| D1 | Does a tmux-backed session have explicit `Link`s (1:1 with panes), or do panes hold a `Screen` endpoint directly? | **Explicit links.** One binding rule ("a pane uses a link") beats two, and it is where exclusivity lives. Cost: a 1:1 link that can never be re-bound. |
| D2 | Window mirroring for tmux sessions: mirror `@N`, or keep one tmux window and treat xscore windows as private? | **Mirror `@N`.** `tmux attach` must show what xscore shows, or the product premise breaks. |
| D3 | `WindowLayout`: delete for good, or keep a hint for native windows? | **Delete.** No consumer; reintroduce with the renderer. |
| D4 | tmux socket: private `-L xscore`, or the user's default socket? | **Default socket**, `--socket` to override. Sessions invisible in `tmux ls` are not "managed". Tests pass `--socket` explicitly. |
| D5 | `link del` on a tmux-backed link | **Forget the handle only.** A link is a view, not an owner. |
| D6 | Can a pane move between sessions? | **No implicit moves.** `pane move --session` is explicit and native-only. |
| D7 | `session forget` — needed, or is `kill` enough? | **Keep it.** Mirrors v0.5.1's `del` vs `close`: destroy vs stop managing. |
| D8 | Alternative to mirroring: address tmux directly (`data --tmux work:1`) and have no xscore rows for tmux at all | **Rejected for now**, because it gives two addressing modes for `data`. Revisit if per-session refresh proves troublesome. |
| D9 | Does xscore pass `ControlMaster=auto` by default, or only when the user asks? | **Pass it**, overridable via `--ssh-arg`. Without it a burst of commands pays a full handshake each time; with it xscore writes no pooling code. |
| D10 | Should `ssh://` links move to the system `ssh` too, leaving one SSH mechanism? | **Not in this change.** `russh` gives a byte stream and in-process control; a link is not an exec client. Revisit only if storing password auth becomes a liability. |

## 13. What this does not solve

- **tmux has no byte stream.** `Screen` endpoints cannot carry binary, exact byte
  order, or exit codes. Anything needing that must be a native session.
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
- **Daemon idle exit still drops native pane buffers** (they are not persisted).
