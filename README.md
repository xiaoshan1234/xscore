# xscore

A session manager for shell, ssh, serial and tmux sessions, built on four
addressable things (ADR 0010):

```
Session ── a workspace, and the backend that keeps it alive
  ├ Window ── a layout container (mirrors a tmux window)
  ├ Pane   ── a buffer with a viewport; the unit you read and write
  └ Link   ── one connection: a pipe, or the session's tmux control client
```

A **native** session is kept alive by xscore's own daemon: its links are pty /
serial / ssh streams that xscore owns. A **tmux** session IS a tmux session:
xscore holds exactly one `tmux -CC attach` client for it, streams every pane's
bytes out of that one connection, and mirrors the structure tmux reports.

Both backends end in the same place — independent ring Buffers in the daemon — so
`xscore data read/write` has one implementation and one meaning.

> **v0.6.0 — what changed.** There is a `Session` tier (a workspace that owns
> windows, panes and links; ADR 0010). tmux is no longer a per-link option:
> `?tmux=on` is gone, `session new --tmux` / `session attach` make a tmux
> session, and it is driven through **control mode** (`tmux -CC`), so a pane is
> a byte stream rather than a `capture-pane` snapshot. The daemon now owns tmux
> control clients too, which is why pane data survives the CLI process.
> `link sync`, `link open/close`, `WindowLayout` and the `Sink` layer are gone.

There are two supported Windows shapes. The native `xscore.exe` can own a WSL,
PowerShell or `pwsh` shell through `shell:///...`, and bridges the daemon over a
Windows named pipe. Every shell-family URL is the same PTY/ConPTY Shell
transport with a different program and argv. Running xscore itself inside WSL
remains the way to manage local or remote tmux sessions on the WSL side.

## 5-minute demo

```bash
# 1. Build (one-time). Rust 1.95+, tokio.
cargo build --release

# 2. A raw Session requires at least one Link. The SessionManager converts
#    Link bytes into independent Buffers and creates the initial UI Pane.
./target/release/xscore session new dev \
  --link 'shell:///bin/bash?cwd=/tmp&cols=120&rows=40'
# stdout:
#   session: 0192...a1
#   link:    0192...a2
#   buffer:  0192...a5
#   window:  0192...a3
#   pane:    0192...a4

# 3. Write and read through the pane (two separate CLI invocations).
echo 'echo hi from xscore' | ./target/release/xscore data write --pane <pid>
./target/release/xscore data read --pane <pid>
# hi from xscore
#
# The Link's SessionManager handed the pty and buffer to the
# resident daemon (ADR 0009), which keeps pumping while no CLI is running.
# `data read` returns the bytes that arrived, never a screen.

# 4. A tmux session. tmux creates the first window and pane; xscore adopts
#    them by handle (@0 / %0).
./target/release/xscore session new work --tmux
#   session: ... / link: ... / window: ... / pane: ...

# 5. A second tmux window and a split inside it — real tmux operations,
#    issued over the session's control client.
./target/release/xscore window add --session work --name logs
./target/release/xscore pane add --session work --window <wid>

# 6. Someone else splits a pane in tmux. The next command sees it,
#    because structure is read from tmux, not cached:
tmux split-window -t work
./target/release/xscore pane list --session work     # one more pane

# 7. What the daemon holds right now (JSON on stdout, a human line on stderr):
./target/release/xscore daemon status
# tmux <session>: work alive=true panes=2 buffered=1536
# {"ok":true,"status":{"pid":1234,"links":[],"buffers":[],
#  "tmux":[{"session":"<sid>","name":"work","alive":true,"panes":2,"buffered":1536}]}}

# 8. Kill the session: the only cascade in the model. For a tmux session
#    that means `kill-session`, and the control client goes with it.
./target/release/xscore session kill work
```

### Windows quick start

Run this from PowerShell when `wsl.exe` is installed:

```powershell
cargo build --release
.\target\release\xscore.exe session new dev `
  --link 'shell:///C:/Windows/System32/wsl.exe?cols=120&rows=40'

# Use the pane id printed above. The shell runs in WSL; the daemon runs on
# Windows and is reached over a per-user named pipe.
'echo hi from WSL' | .\target\release\xscore.exe data write --pane <pid>
.\target\release\xscore.exe data read --pane <pid>

# Pick a distribution, shell and WSL directory explicitly.
.\target\release\xscore.exe session new ubuntu `
  --link 'shell:///C:/Windows/System32/wsl.exe?arg=--distribution&arg=Ubuntu&arg=--cd&arg=/home/me/src&arg=--exec&arg=/bin/bash'

# Native Windows PowerShell 5.1 or PowerShell 7
.\target\release\xscore.exe session new ps `
  --link 'shell:///C:/Windows/System32/WindowsPowerShell/v1.0/powershell.exe?arg=-NoLogo&cwd=C:/src'
.\target\release\xscore.exe session new ps7 `
  --link 'shell:///C:/Program%20Files/PowerShell/7/pwsh.exe?arg=-NoLogo&cwd=C:/src'
```

## The model

```
Session 1:N Window      Window  1:N Pane       Pane 0..1 Buffer (pure UI reference)
Session 1:N Link        Session 1:N Pane       Pane 0..1 Buffer (pure UI reference)
Session 1:N Buffer      Link 1:N Buffer        UI Pane   0..1 Buffer
```

| | `Backend::Native` | `Backend::Tmux { host, socket, session }` |
|---|---|---|
| keeps processes alive | the daemon | the tmux server |
| Window | xscore's own grouping | mirrors a tmux window (`@N`) |
| Pane | xscore buffer + geometry | is a tmux pane (`%N`) |
| Link | `Pipe` — xscore owns the pty/device/channel | `Control` — the session's one client |
| links per session | one per connection you opened | exactly one |
| attaches a human | n/a | `session attach` → `exec tmux attach` |

`session new` creates a Session with at least one Link. A backend-specific
SessionManager converts Link input into independent Buffers. Panes are pure UI
viewports that reference Buffers; they do not own bytes or Links.

**Exclusivity is per link kind.** A `Pipe` link serves at most one pane — two
readers on one pty would steal each other's bytes. A `Control` link serves every
pane of its session, because tmux names the recipient of each chunk (`%output %N`);
sharing there is addressing, not a race. A tmux pane therefore cannot be moved to
another link, and `link del` is refused for a control link.

## Where the bytes live

| object | owner | dies when |
|---|---|---|
| session / window / pane / buffer / link rows | CLI → `state.json` | `session kill`, `link del`, `pane del` |
| pipe transports (pty child / serial fd / ssh channel) | daemon | `link del`, `daemon stop` |
| **tmux control client** (`tmux -CC attach`) | daemon | `session kill/detach`, `daemon stop` |
| Buffer ring data (64 KiB, both kinds) | daemon | `buffer del`, `daemon stop` (memory only) |
| tmux structure (windows, panes, layout) | tmux | tmux |

The CLI is one process per command; anything that must outlive it lives in the
daemon. That is why `data write` in one terminal and `data read` in the next
work, and why `daemon stop` loses buffered bytes while leaving `state.json` and
the tmux server untouched.

The daemon starts on demand, and the copy that auto-start spawns in the
background leaves after 5 s with nothing to hold. A `xscore daemon serve` you
run yourself is the opposite: it stays up until `daemon stop`, Ctrl-C or
SIGTERM.

## tmux through control mode

`session new --tmux` runs `tmux -L <sock> new-session -d`, then attaches one
`tmux -CC attach` client (over `ssh <dest>` for a remote host). That client is
the session's single link:

```
tmux server ──%output %N, %window-add, %layout-change, %exit──► TmuxControl
                                                                  │
                              ┌───────────────────────────────────┤
                              ▼                                   ▼
                    pane %N's ring buffer                 structure queries
```

Measured on tmux 3.5a, and the reasons the details matter:

- **`-CC` with a dedicated PTY.** `-CC` disables echo by calling `tcgetattr` on the client's
  terminal, so with pipes it dies with `tcgetattr failed: Inappropriate ioctl for
  device`. That form is for clients that own a real terminal (iTerm2 allocates a
  pty); `-C` gives a pipe client the same protocol.
- **Replies are matched by order.** tmux picks `%begin`'s number itself (a
  counter in the hundreds), so a client that waits for its own numbering hangs.
- **Pane bytes are escaped text on a line protocol**: `\ooo` is an octal byte,
  `\\` a backslash, and newlines are escaped, so one line is one complete record.
  `\0` cannot be sent; writes carrying a newline go through `send-keys -H <hex>`.
- **Nothing is replayed.** Output produced before the client attached is gone;
  tmux's scrollback is not the stream. A pane discovered late has no history.
- **Structure is read on touch.** A one-shot CLI has nobody to push events to, so
  `window list` / `pane list` ask the control client for the current structure
  (one round trip, no process spawn). Events are used for liveness (`%exit`) and
  to drop a closed pane's buffer.

## Commands

Choose the guide for your runtime:

- [`doc/user-guide/windows.md`](doc/user-guide/windows.md): native
  `xscore.exe`, PowerShell 5.1/7, WSL shells, ConPTY and the Windows named-pipe
  daemon.
- [`doc/user-guide/linux.md`](doc/user-guide/linux.md): Linux or xscore running
  inside WSL, including native shells, tmux, serial, SSH and the Unix-socket
  daemon.

The complete command and flag reference is
[`doc/user-guide/commands.md`](doc/user-guide/commands.md). The platform guides
include setup, pane read/write examples and troubleshooting.
Below is the shape of the surface.

```bash
# Sessions require a Link. Raw sessions accept repeatable --link; tmux creates
# its one control Link internally.
xscore session new <name> --link <URL> [--link <URL> ...]
xscore session new <name> --tmux [--ssh DEST] [--socket S]
xscore session attach <name> [--socket S] [--ssh DEST]   # attach to an existing tmux session
xscore session list                                     # windows / panes / buffers / links per session
xscore session show <name>                              # the ids it owns
xscore session kill <name>                              # the only cascade
xscore session detach <name>                            # drop the rows, leave tmux running
xscore session rename <old> <new>

# Links — every add explicitly names its owning session
xscore link add <URL> --session <s>

# Buffers — independent byte owners referenced by UI panes
xscore buffer list [--session <s>]
xscore buffer show --id <bid>
xscore buffer del --id <bid>

# Windows — layout containers
xscore window add --session <s> [--name N] [--cols C --rows R]
xscore window list [--session <s>]
xscore window show --id <wid>
xscore window del --id <wid>                            # panes survive as orphans
xscore window add-pane --window <wid> --pane <pid>
xscore window remove-pane --window <wid> --pane <pid>

# Panes — buffers with a viewport
xscore pane add --session <s> [--window <wid>] [--name N] [--cols C --rows R]
xscore pane list [--session <s>] [--window <wid>] [--buffer <bid>]
xscore pane show --id <pid>
xscore pane del --id <pid>                              # UI row only; Buffer and source survive
xscore pane attach --pane <pid> --link <lid>            # reference the Buffer produced by a raw Link
xscore pane detach --pane <pid>                         # clear the UI reference; Buffer kept
xscore pane resize --pane <pid> --cols C --rows R
xscore pane move --pane <pid> --x X --y Y               # geometry only

# Links — one connection each
xscore link add <url> --session <s> [--id <uuid>] [--pane <pid>]
xscore link list [--session <s>]
xscore link show --id <lid>                             # includes "connected"
xscore link del --id <lid>                              # pipe links only
xscore link config {add,remove,show,list}               # persistent URL templates

# Data — any pane, native or tmux
xscore data read  --pane <pid> [--bytes N]
xscore data write --pane <pid> [< file | --from-file F]
xscore data resize --pane <pid> --cols C --rows R

# Daemon and state
xscore daemon serve | status | stop
xscore state migrate        # v2 → v3, and old tmux links → control sessions
```

### URL syntax (pipe links)

| Scheme | Form | Example |
|---|---|---|
| `shell://` | an executable path in a PTY/ConPTY | `shell:///bin/bash` |
| `shell:///C:/Windows/System32/wsl.exe` | WSL shell | `shell:///C:/Windows/System32/wsl.exe` |
| `shell:///.../powershell.exe` | Windows PowerShell 5.1 | `shell:///C:/Windows/System32/WindowsPowerShell/v1.0/powershell.exe` |
| `shell:///.../pwsh.exe` | PowerShell 7 | `shell:///C:/Program%20Files/PowerShell/7/pwsh.exe` |
| `serial://` | serial device | `serial:///dev/ttyUSB0?baud=115200&data_bits=8&parity=n&stop_bits=1` |
| `ssh://` | SSH channel (key auth) | `ssh://user@host:22?identity=~/.ssh/id_ed25519` |

tmux is not a URL scheme. A tmux session is a workspace: `session new --tmux`
(or `session attach`), not `link add`.

All shell-family URLs normalize to `TransportConfig::Shell { command, args,
cwd }`; WSL and PowerShell do not add transport variants. The path after
`shell:///` is always the child executable path. Repeat `arg=` to pass
child-process arguments; use `cwd`, `cols`, `rows` and `name` for generic pane
and process settings. For WSL, pass `wsl.exe` options through repeated `arg=`
parameters, including `--distribution`, `--exec` and `--cd`.

## Architecture

Source dependencies flow through five layers:

```text
app/cli                    user input and command dispatch
  -> service               coordination and Raw/Tmux conversion
       -> ui               Window/Pane rows and UiEvent application
       -> infra            pty/ssh/serial, tmux -CC and daemon I/O
  -> domain                shared IDs, errors and persisted model
```

The UI layer is currently headless and only builds core Window/Pane rows. A
Pane references a Buffer; it never owns a Link or a byte stream. Raw/Tmux
SessionManagers decode Link input into Buffer writes and UI events. Link-layer
infrastructure owns the actual connection.

```
CLI (one process per command)                    daemon (resident)
  SessionManager                                   links  : LinkId → Transport
    ├ SessionRegistry   (session_id → Session)     raw    : Link/Buffer routes + pumps
    ├ SessionManagers   (Raw routes 1..N Links)    buffers: BufferId → RingBuffer
    ├ LinkRegistry      (link_id → Link)           tmux   : SessionId → ControlSlot
    ├ BufferRegistry    (buffer_id → BufferInfo)            { control, buffers, alive }
    ├ WindowRegistry    (window_id → Window)
    ├ PaneRegistry      (pane_id → UI PaneInfo)
    └ state.json        (rows only, no bytes)
            │
            └── local IPC, length-prefixed JSON ──┘
```

The local IPC endpoint is a Unix socket on Linux and a per-user Windows named
pipe on Windows. The request protocol and daemon state machine are shared.

The CLI owns metadata and is the only writer of `state.json`. The daemon owns
every live connection — pipe transports *and* tmux control clients — and the
buffers those connections feed. The split is the whole point: a command can be
short-lived, a connection cannot.

A pane reaches its bytes through a `PaneBridge`:

- `DaemonBridge` — the normal path: read/write/resize are requests to the daemon,
  which routes to a pipe transport or to `send-keys` / `resize-pane` on the
  session's control client.
- `DirectBridge` — in-process (tests, and `SessionManager::new()` with no daemon).
- `ControlBridge` — in-process tmux: the buffer is filled by the local client.

## Status (v0.6.0)

**Working end-to-end, with tests:**

- The four-tier model: sessions own windows, panes and links; every child carries
  `session_id`; `session kill` is the only cascade.
- Native sessions on real ptys: `TERM` set, `isatty` true, echo and job control
  work, `data resize` reaches `TIOCSWINSZ`.
- tmux sessions through control mode: one link per session, `%output %N` streamed
  into independent Buffers, `send-keys` writes with exact escaping, structure read
  from tmux on touch, `session attach` for a session someone else created.
- The daemon owns both kinds of live connection, so `data read` is a *stream*
  across CLI invocations (`daemon status` shows the tmux slot and its byte count).
- `state migrate` upgrades a v2 file, and converts old `?tmux=on` links into
  control-mode sessions instead of dropping them.
- URL parsing, `link config` templates, Q1 (one pane per pipe link), pane geometry,
  per-session filters on every list.
- Native Windows daemon IPC over named pipes, plus WSL and PowerShell shells
  launched through ConPTY. Raw/Tmux SessionManagers own Link-to-Buffer routing.

**Implemented but not exercised by the suite:** `serial://` (needs hardware) and
`ssh://` links (needs a server; key auth only, `check_server_key` accepts any key).

**Stubbed:** `xscore config {show,path,set,unset}`.

**Known limits:**

- Native Windows supports WSL and PowerShell shell links. Local tmux on the
  Windows side is not supported; manage WSL tmux by running xscore inside WSL.
- Pane buffers live in daemon memory: `daemon stop` loses them (metadata and tmux
  survive). Direct link transports die with the daemon too; `link show` reports
  `connected: false` and `pane attach` rebuilds one.
- A tmux pane discovered late has no history — tmux replays nothing.
- `data write` to a tmux pane needs UTF-8 text; `\0` is impossible on a line
  protocol. Payloads with newlines go through `send-keys -H`.
- Concurrent mutating CLI calls still last-writer-wins on `state.json`.
- A remote tmux session's ssh credentials come from the daemon's environment, so
  a key added after the daemon started is not seen until `daemon stop`.
- No remote *native* sessions: a `Backend::Native` session runs on this machine.

## Tests

```bash
cargo test --workspace --all-targets -- --test-threads=1
cargo test -p xscore --test tmux_control_smoke -- --ignored # live tmux, Unix only
```

Windows runs the library suite, a named-pipe protocol test, a ConPTY test and
`windows_smoke` for daemon status/stop. The Unix shell and tmux integration files
are platform-gated; run them on Linux or inside WSL.

On Unix, `cli_smoke` is deliberately cross-process: create in A, write in B,
read in C. `data_smoke` runs without a daemon on purpose, to keep
`DirectBridge` honest.
`tmux_control_smoke` is ignored by default because it needs `tmux` on `PATH`; it
uses a private socket and kills it on the way out.

## File layout

```
crates/xscore/src/
  app/
    cli/                 command parsing and dispatch; mcp/ is reserved
  ui/
    window.rs            Window UI row and registry
    pane.rs              Pane UI row and registry
    event.rs             events consumed by the headless UI model
  service/
    buffer_manager/      Buffer rows, ring history and PaneBridge adapters
    ui_manager.rs        UI registry coordinator
    link_manager/        Link rows and connection ownership
    session_manager/     orchestration, Raw/Tmux backends and persistence
  infra/
    link/                pty, ssh, serial and transport factory
    tmux/                control-mode client, parser and text formats
    daemon/              resident transports/Buffers process and IPC
    config/, url/, util/
  domain/                shared IDs, errors and persisted model types
crates/xscore/tests/
  cli_smoke.rs, data_smoke.rs, tmux_control_smoke.rs, windows_smoke.rs
```

## Release process

See [`RELEASE.md`](RELEASE.md). Design decisions live in [`doc/dev/adr/`](doc/dev/adr/);
the target architecture is [`doc/dev/arch/arch.md`](doc/dev/arch/arch.md).

## License

MIT (see `LICENSE`).
