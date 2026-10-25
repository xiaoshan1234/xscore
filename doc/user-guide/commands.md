# xscore command reference

This document lists every command exposed by the current `xscore` binary and
every command-specific flag. The platform walkthroughs remain:

- [`windows.md`](windows.md)
- [`linux.md`](linux.md)

The command help is the final source of truth. Recheck a command with:

```text
xscore <command> --help
xscore <group> <command> --help
```

## Conventions

- IDs are UUIDv7 strings. Copy them from `session new`, `session show`,
  `session list`, `link list`, `window list` or `pane list`.
- Session names resolve session-scoped commands and filters.
- Exit status is `0` on success and non-zero on failure.
- JSON and pane data are written to stdout. Diagnostics are written to stderr.
- `-h` / `--help` is accepted at every command level.
- `-V` / `--version` is accepted by the top-level command.

Top-level surface:

```text
xscore
├── session  list · new · attach · show · kill · detach · rename
├── link     add · del · show · list · config {add,remove,show,list}
├── window   add · list · show · del · add-pane · remove-pane
├── pane     add · del · show · list · attach · detach · resize · move
│            add-to-window · remove-from-window
├── data     read · write · resize
├── daemon   serve · status · stop
├── state    migrate
├── config   stub
└── help     generated help
```

## URL grammar

### Shell links

```text
shell:///<absolute-executable-path>[?arg=...&cwd=...&cols=...&rows=...&name=...]
```

`shell` is the protocol. `/absolute-executable-path` is the executable to run
behind a PTY or Windows ConPTY.

Shell query parameters:

| Key | Meaning |
|---|---|
| `arg` | One child-process argument. Repeat `arg=` for multiple arguments. |
| `cwd` | Working directory of the spawned process. |
| `cols` | Initial terminal columns. Must be non-zero. |
| `rows` | Initial terminal rows. Must be non-zero. |
| `name` | Display name of the created link. |

Examples:

```text
shell:///bin/bash
shell:///bin/bash?arg=-l&cwd=/home/me/src&cols=120&rows=40

shell:///C:/Windows/System32/WindowsPowerShell/v1.0/powershell.exe?arg=-NoLogo
shell:///C:/Program%20Files/PowerShell/7/pwsh.exe?arg=-NoLogo

shell:///C:/Windows/System32/wsl.exe?arg=--distribution&arg=Ubuntu&arg=--cd&arg=/home/me/src&arg=--exec&arg=/bin/bash
```

Notes:

- URL paths are percent-decoded, so spaces in a Windows path can be written as
  `%20`.
- A Windows drive path may be written as `/C:/...`.
- Shell authority is not supported: use `shell:///bin/bash`, not
  `shell://bash`.
- `distro` and `shell` are not special shell query keys. WSL options are passed
  as repeated `arg=` values.
- For WSL, `cwd` changes the Windows-side `wsl.exe` process directory. Use
  `arg=--cd` to change the Linux working directory.

### Serial links

```text
serial:///<device>?baud=<rate>[&data_bits=...&parity=...&stop_bits=...&cols=...&rows=...]
```

| Key | Required | Default |
|---|---|---|
| `baud` | yes | none |
| `data_bits` | no | `8` |
| `parity` | no | `n` |
| `stop_bits` | no | `1` |
| `cols`, `rows` | no | pane geometry |
| `name` | no | generated link name |

Example:

```text
serial:///dev/ttyUSB0?baud=115200&data_bits=8&parity=n&stop_bits=1
```

### SSH links

```text
ssh://<user>@<host>[:port][?identity=...&proxy=...&cols=...&rows=...&name=...]
```

SSH links currently use key authentication through `russh`.

Example:

```text
ssh://me@host:22?identity=~/.ssh/id_ed25519
```

Current limitation: the `ssh://` transport accepts any server host key and does
not yet verify `known_hosts`.

## session

### `session list`

```text
xscore session list
```

Lists sessions and their window, pane and link counts.

### `session new`

```text
xscore session new [OPTIONS] <NAME>
```

Creates the Session container. A native session starts empty. `--tmux` selects
the tmux backend and lets tmux create its default window/pane.

Arguments:

| Argument | Meaning |
|---|---|
| `<NAME>` | New session name. |

Options:

| Option | Meaning |
|---|---|
| `--tmux` | Create a real tmux session with one control link. |
| `--ssh <DEST>` | Create the tmux server on this SSH host. Requires `--tmux`. |
| `--socket <NAME>` | tmux socket (`tmux -L NAME`). Defaults to `$XSCORE_SOCKET`, then the default server. |

Examples:

```text
xscore session new dev
xscore session new work --tmux
xscore session new build --tmux --ssh dev@box
```

Session selection rules for child-creating commands:

```text
--session NAME       NAME must already exist
no --session + 1     use the only Session
no --session + 0     error: run `xscore session new <name>` first
no --session + N>1   error: specify `--session <name>`
```

No command creates an implicit `default` Session.

### `session attach`

```text
xscore session attach [OPTIONS] <NAME>
```

Attaches to an existing tmux session under the same visible name.

| Option | Meaning |
|---|---|
| `--socket <NAME>` | tmux socket. Defaults to `$XSCORE_SOCKET`, then the default server. |
| `--ssh <DEST>` | tmux server runs on this SSH destination. |

### `session show`

```text
xscore session show <NAME>
```

Prints the session backend and IDs of its windows, panes and links.

### `session kill`

```text
xscore session kill <NAME>
```

Destroys the session and all children. This is the model's only cascade. For a
tmux session, it runs `kill-session`.

### `session detach`

```text
xscore session detach <NAME>
```

tmux only. Drops xscore state and leaves the tmux session running. Native
sessions use `session kill`.

### `session rename`

```text
xscore session rename <OLD> <NEW>
```

Renames a session. Existing names are rejected.

## link

A link is one connection. `link add` creates a pipe link. A tmux session's one
control link is created by `session new --tmux` or `session attach`.

### `link add`

```text
xscore link add [OPTIONS] [URL]
```

Creates a pipe link and normally creates its first window and pane.

| Option | Meaning |
|---|---|
| `-c, --config <CONFIG>` | Use a saved link template. |
| `--id <UUID>` | Use this link ID instead of generating one. |
| `--pane <PANE_ID>` | Bind an existing orphan pane instead of creating a new one. |
| `--session <NAME>` | Session that owns the link. |

Examples:

```text
xscore link add 'shell:///bin/bash' --session dev
xscore link add 'serial:///dev/ttyUSB0?baud=115200' --session hw
xscore link add -c prod --session ops
```

The Session must already exist. If `--session` is omitted, xscore uses the only
existing Session; it errors when none or multiple Sessions exist. It never
creates a `default` Session implicitly.

`link add` is refused inside a tmux session because that session already has
its single control link.

### `link del`

```text
xscore link del --id <ID>
```

Closes a pipe link and orphans its attached panes. Buffers remain. Control
links cannot be deleted; use `session kill` or `session detach`.

### `link show`

```text
xscore link show --id <ID>
```

Prints `LinkInfo` JSON, including `connected` for pipe links.

### `link list`

```text
xscore link list [--session <NAME>]
```

Lists link ID, name, and kind. Examples of kind:

```text
pipe/shell
pipe/serial
pipe/ssh
control/shell
control/ssh
```

### `link config add`

```text
xscore link config add <NAME> <URL>
```

Saves a URL template under the xscore config directory.

### `link config remove`

```text
xscore link config remove <NAME>
```

Deletes a saved template.

### `link config show`

```text
xscore link config show <NAME>
```

Prints a saved template.

### `link config list`

```text
xscore link config list
```

Lists saved template names.

## window

### `window add`

```text
xscore window add [OPTIONS]
```

Creates a native window or a real tmux window.

| Option | Default | Meaning |
|---|---|---|
| `--name <NAME>` | generated or tmux name | Window name. |
| `--cols <COLS>` | `120` | Native window width. |
| `--rows <ROWS>` | `30` | Native window height. |
| `--session <NAME>` | only existing session | Owning session. No Session is created implicitly. |

### `window list`

```text
xscore window list [--session <NAME>]
```

Lists windows with IDs, names, session, geometry and tmux handle.

### `window show`

```text
xscore window show --id <ID>
```

Prints `WindowInfo` JSON.

### `window del`

```text
xscore window del --id <ID>
```

Removes the window row. Panes survive as orphans. For a tmux window this kills
the tmux window.

### `window add-pane`

```text
xscore window add-pane --window <WINDOW> --pane <PANE>
```

Adds an existing pane to a window.

### `window remove-pane`

```text
xscore window remove-pane --window <WINDOW> --pane <PANE>
```

Removes the association. The pane survives as an orphan.

## pane

### `pane add`

```text
xscore pane add [OPTIONS]
```

Creates a pane. Without `--window`, it is an orphan with no link. With a tmux
window, it creates a real tmux split.

| Option | Default | Meaning |
|---|---|---|
| `--name <NAME>` | generated | Pane name. |
| `--cols <COLS>` | `80` | Width. |
| `--rows <ROWS>` | `24` | Height. |
| `--x <X>` | `0` | Horizontal position. |
| `--y <Y>` | `0` | Vertical position. |
| `--window <WINDOW_ID>` | none | Put the pane in this window. |
| `--session <NAME>` | only existing session | Owning session. No Session is created implicitly. |

### `pane del`

```text
xscore pane del --id <ID>
```

Fully deletes the pane, detaching it from its link and window. A tmux pane is
also killed.

### `pane show`

```text
xscore pane show --id <ID>
```

Prints `PaneInfo` JSON, including `link_id`, `window_id`, `link_key` and
geometry.

### `pane list`

```text
xscore pane list [OPTIONS]
```

| Option | Meaning |
|---|---|
| `--session <NAME>` | Only this session. |
| `--window <WINDOW>` | Only panes in this window. |
| `--link <LINK>` | Only panes on this link. |

### `pane attach`

```text
xscore pane attach --pane <PANE> --link <LINK>
```

Binds a pane to a pipe link. The link's previous pane is auto-detached because a
pipe has one reader. This is also the reconnect operation for an already-bound
pane whose daemon transport died.

### `pane detach`

```text
xscore pane detach --pane <PANE>
```

Unbinds a pipe pane. The pane survives and keeps its buffer. tmux panes cannot
be detached.

### `pane resize`

```text
xscore pane resize --pane <PANE> --cols <COLS> --rows <ROWS>
```

Updates geometry and forwards resize to an attached bridge.

### `pane move`

```text
xscore pane move --pane <PANE> --x <X> --y <Y>
```

Updates metadata coordinates only.

### `pane add-to-window`

```text
xscore pane add-to-window --pane <PANE> --window <WINDOW>
```

Associates an existing pane with a window.

### `pane remove-from-window`

```text
xscore pane remove-from-window --pane <PANE> --window <WINDOW>
```

Removes the window association. The pane survives.

## data

### `data read`

```text
xscore data read [OPTIONS] --pane <PANE>
```

| Option | Default | Meaning |
|---|---|---|
| `--pane <PANE>` | required | Pane to read. |
| `--bytes <BYTES>` | `4096` | Maximum number of newest buffer bytes to return. |

This is a non-destructive snapshot, not a queue. Reading twice returns the same
buffer contents. Raw terminal bytes may include echoes, `\r` and ANSI escapes.

### `data write`

```text
xscore data write [OPTIONS] --pane <PANE>
```

| Option | Meaning |
|---|---|
| `--pane <PANE>` | Required target pane. |
| `--from-file <FROM_FILE>` | Read bytes from this file instead of stdin. |

Examples:

```bash
printf 'ls -la\n' | xscore data write --pane <pid>
xscore data write --pane <pid> --from-file ./commands.txt
```

A shell typically executes only after receiving a line ending. Control keys are
raw bytes (`0x03` for Ctrl-C, `0x04` for Ctrl-D). tmux panes accept UTF-8 text
and cannot receive NUL.

### `data resize`

```text
xscore data resize --pane <PANE> --cols <COLS> --rows <ROWS>
```

Resizes through the pane bridge. This errors when the pane has no live
transport. Use `pane resize` for metadata-only geometry.

## daemon

### `daemon serve`

```text
xscore daemon serve [--idle-exit]
```

Runs the resident daemon in the foreground.

| Option | Meaning |
|---|---|
| `--idle-exit` | Exit after 5 idle seconds with no links or tmux clients. Auto-start uses this mode. A bare `serve` stays up until stopped. |

### `daemon status`

```text
xscore daemon status
```

Prints JSON containing:

```text
pid
links[]
panes[] with link id and buffered byte count
tmux[] with name, alive, pane count and buffered bytes
```

Exits non-zero when no daemon is running.

### `daemon stop`

```text
xscore daemon stop
```

Stops the daemon, closes live transports and tmux control clients, and loses
in-memory pane buffers. Session metadata and tmux itself survive.

## state

### `state migrate`

```text
xscore state migrate
```

Upgrades `state.json` in place:

- v2 to schema v3 by adding the session tier;
- old v3 tmux-link rows to control-mode sessions.

The command is idempotent.

## config

```text
xscore config
```

Currently a stub. It exits non-zero because AppConfig show/path/set/unset are
not implemented.

## Environment and state

| Variable | Used for |
|---|---|
| `HOME` | Config root; otherwise `USERPROFILE` on Windows. |
| `XDG_RUNTIME_DIR` | Unix daemon directory when set. |
| `XSCORE_SOCKET` | Default tmux socket for `--tmux` and `session attach`; `--socket` wins. |
| `PATH` | Program lookup for shell executables, `tmux` and system `ssh`. |

Default state:

```text
~/.config/xscore/state/state.json
~/.config/xscore/links/
~/.config/xscore/run/daemon.sock
```

On Windows, the same `.config/xscore` layout is based on `HOME` or
`USERPROFILE`, while daemon IPC uses a per-user named pipe.

## Command families at a glance

| Family | Purpose |
|---|---|
| `session` | Top-level workspaces and backend lifetime. |
| `link` | Connections: shell, serial, SSH or a tmux control client. |
| `window` | Grouping containers and tmux window projections. |
| `pane` | Buffers, viewport geometry and link/window associations. |
| `data` | Read, write and resize pane bytes. |
| `daemon` | Inspect or control the process that owns live I/O. |
| `state` | Migrate persisted metadata. |
| `config` | Reserved AppConfig command, currently unimplemented. |
