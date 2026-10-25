# xscore on Linux

This guide covers xscore running in Linux or inside WSL. The CLI owns session
metadata, while a resident Unix-socket daemon owns live PTYs, serial devices,
SSH channels, tmux control clients and pane buffers.

For the native Windows executable, full PowerShell/WSL executable paths and
the Windows named-pipe daemon, read [`windows.md`](windows.md).

For every command and flag, read [`commands.md`](commands.md).

## Requirements

- Rust 1.95 or newer to build from source.
- `tmux` 3.0 or newer on `PATH` for tmux sessions.
- `ssh` on `PATH` for remote tmux sessions.
- Access to a serial device only if you use `serial://`.

## Build

```bash
cargo build --release
./target/release/xscore --help
```

Install it on `PATH`:

```bash
install -m755 target/release/xscore ~/.local/bin/xscore
```

The examples below use `xscore`; use `./target/release/xscore` if you have not
installed it.

## Model

```text
Session  a workspace with a Native or Tmux backend
  Window a grouping container; a tmux window mirrors @N
  Pane   a byte buffer and geometry; the unit of read/write
  Link   one connection: shell/ssh/serial, or a tmux control client
```

A native session is kept alive by the xscore daemon. A tmux session is kept
alive by the tmux server. In both cases, pane bytes are held in daemon memory
and survive across separate CLI invocations.

## Create a Session and add a shell link

Create an empty native Session:

```bash
xscore session new dev
```

Child-creating commands require an existing Session. If exactly one Session
exists, `--session` may be omitted; when none or multiple exist, the command
fails instead of creating or guessing a `default`.

Add a shell Link inside it:

```bash
xscore link add \
  'shell:///bin/bash?cwd=/home/me/src&cols=120&rows=40' \
  --session dev
```

When xscore itself runs inside WSL, `shell:///bin/bash` starts a shell in that
same WSL instance. The Windows path to `wsl.exe` belongs to the native Windows
guide and is not needed from inside WSL.

### URL syntax

```text
shell:///<program>[?arg=...&cwd=...&cols=...&rows=...&name=...]
serial:///<device>?baud=<rate>[&data_bits=...&parity=...&stop_bits=...]
ssh://<user>@<host>[:port][?identity=...&proxy=...]
```

The path after `shell:///` is the shell program. Shell query parameters:

| Key | Meaning |
|---|---|
| `arg` | One child-process argument. Repeat it for each argument. |
| `cwd` | Working directory of the child process. |
| `cols`, `rows` | Initial terminal size. |
| `name` | Link display name. |

Unknown query keys are errors.

A login Bash shell:

```bash
xscore session new login
xscore link add 'shell:///bin/bash?arg=-l' --session login
```

## Find session and pane ids

```bash
xscore session show dev
xscore session list
xscore pane list --session dev
```

`session new` prints the Session id. `link add` prints the child ids:

```text
session: 01a...
```

```text
link:    01a...
window:  01a...
pane:    01a...
```

Capture the first pane id in a script:

```bash
xscore session new dev

out=$(xscore link add 'shell:///bin/bash' --session dev)
printf '%s\n' "$out"

pane=$(printf '%s\n' "$out" | awk '/^pane:/{print $NF}')
link=$(printf '%s\n' "$out" | awk '/^link:/{print $NF}')
```

## Read from a pane

```bash
xscore data read --pane "$pane" --bytes 4096
```

`data read` returns a non-destructive snapshot of the newest requested bytes.
Reading twice returns the same buffer contents; nothing is consumed.

The output is raw terminal data. It may contain echoed input, `\r`, prompts and
ANSI escapes. To remove common terminal escapes:

```bash
xscore data read --pane "$pane" |
  sed 's/\x1b\[[0-9;?]*[a-zA-Z]//g' |
  tr -d '\r'
```

## Write to a pane

A newline is Enter:

```bash
printf 'echo hello from xscore\n' |
  xscore data write --pane "$pane"
```

Read the result:

```bash
sleep 0.3
xscore data read --pane "$pane"
```

`printf 'echo hi'` types the bytes but does not execute the command.
`printf 'echo hi\n'` executes it.

Write an exact script from a file:

```bash
xscore data write --pane "$pane" --from-file ./commands.sh
```

Control keys are bytes:

```bash
printf '\003' | xscore data write --pane "$pane"  # Ctrl-C
printf '\004' | xscore data write --pane "$pane"  # Ctrl-D
```

Resize the terminal:

```bash
xscore data resize --pane "$pane" --cols 120 --rows 40
```

## Windows, links and panes

```bash
xscore window add --session dev --name logs
xscore window list --session dev

xscore pane add --session dev --window <wid>
xscore pane list --session dev
```

`link add` creates a connection plus a usable first window and pane:

```bash
xscore link add 'shell:///bin/bash' --session dev
xscore link list --session dev
```

A pipe link carries one pane at a time. Attach an orphan pane or move the active
reader:

```bash
xscore pane attach --pane <pid> --link <lid>
xscore pane detach --pane <pid>
```

`pane detach` keeps the pane buffer. `pane attach` is also the reconnect
operation after `daemon stop`.

## tmux

Create a managed tmux session:

```bash
xscore session new work --tmux
```

xscore creates one `tmux -C attach` control client for the session. Every tmux
pane is addressed by its `%N` handle on that one link.

Take over an existing tmux session:

```bash
tmux new-session -d -s build
xscore session attach build
xscore pane list --session build
```

Create or inspect windows and panes:

```bash
xscore window add --session work --name logs
xscore pane add --session work --window <wid>
xscore window list --session work
xscore pane list --session work
```

Structure is read from tmux on demand. A window created externally appears on
the next xscore listing:

```bash
tmux new-window -t work
xscore window list --session work
```

Reading and writing use the same data commands:

```bash
printf 'echo hello from tmux\n' |
  xscore data write --pane <tmux-pane-id>
xscore data read --pane <tmux-pane-id>
```

tmux control mode accepts UTF-8 text only and cannot transmit NUL. Newlines are
supported and sent through `send-keys -H`.

Choose a private tmux socket:

```bash
xscore session new work --tmux --socket scratch
XSCORE_SOCKET=scratch xscore session attach work
```

End a tmux session:

```bash
xscore session kill work    # kills the tmux session
xscore session detach work  # leaves tmux running, drops xscore rows
```

Remote tmux uses the system `ssh` command, so `~/.ssh/config`, `ProxyJump` and
the SSH agent apply:

```bash
xscore session new build --tmux --ssh dev@box
```

Connection options such as port and identity come from `~/.ssh/config` because
the tmux control client uses the system `ssh`.

## Serial and SSH links

Serial:

```bash
xscore session new hw
xscore link add \
  'serial:///dev/ttyUSB0?baud=115200&cols=100&rows=30' \
  --session hw
```

SSH with key authentication:

```bash
xscore session new server
xscore link add \
  'ssh://me@box:22?identity=~/.ssh/id_ed25519' \
  --session server
```

Current limitation: the `russh`-based `ssh://` link accepts any server host key
and does not yet consult `known_hosts`. Use it only with trusted hosts until
host-key verification is implemented.

Save a URL as a reusable template:

```bash
xscore link config add prod \
  'ssh://deploy@prod:22?identity=~/.ssh/id_ed25519'
xscore link add -c prod --session ops
```

## Daemon

The daemon starts on demand:

```bash
xscore daemon status
xscore daemon stop
```

Run it in the foreground for diagnostics:

```bash
xscore daemon serve
```

`daemon stop`:

- loses pane ring buffers;
- closes shell, serial and SSH transports;
- closes tmux control clients;
- keeps `state.json` and the tmux server.

Reconnect a pipe pane after a daemon restart:

```bash
xscore pane attach --pane <pid> --link <lid>
```

## State and environment

| Item | Default |
|---|---|
| Session metadata | `~/.config/xscore/state/state.json` |
| Link templates | `~/.config/xscore/links/` |
| Daemon socket | `$XDG_RUNTIME_DIR/xscore/daemon.sock`, else `~/.config/xscore/run/daemon.sock` |
| tmux server override | `XSCORE_SOCKET` |

If `HOME` is set, xscore uses `$HOME/.config/xscore` as its config root.

Migrate an older state file:

```bash
xscore state migrate
```

## Troubleshooting

### `session not found`

```bash
xscore session list
```

### The pane is not attached

```bash
xscore pane list --session dev
xscore daemon status
```

A `-` in the link column means the pane has no live connection.

### The link transport died

The daemon stopped or restarted. Reconnect:

```bash
xscore pane attach --pane <pid> --link <lid>
```

### tmux output is missing

tmux does not replay output produced before the control client attached. Start
the command after the session is managed.

### Need exact command flags

```bash
xscore --help
xscore session new --help
xscore data write --help
```
