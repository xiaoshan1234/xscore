# Command reference — xscore v0.6.0

Every command the binary accepts, with the exact flags. Generated from
`xscore --help` / `xscore <group> <verb> --help` and checked against the
v0.6.0 build.

Conventions used below:

> New to xscore? Read [`getting-started.md`](getting-started.md) first — this
> file assumes you already know what a session, pane and link are.

- **ids** are UUIDv7 strings. Copy them from `session show`, `link list`,
  `window list`, `pane list`, or from the four lines `session new` prints.
- **`<NAME>`** is always a *session* name — the handle every other verb
  resolves.
- Running any command with `--help` prints the same text this file documents.
- Everything exits `0` on success and non-zero on failure; errors go to
  stderr, data goes to stdout.

```
xscore
├── session  ls · new · adopt · show · kill · forget · rename
├── link     add · del · show · list · config {add,remove,show,list}
├── window   add · list · show · del · add-pane · remove-pane
├── pane     add · del · show · list · attach · detach · resize · move
│            add-to-window · remove-from-window
├── data     read · write · resize
├── daemon   serve · status · stop
├── state    migrate
└── config   (stub — not implemented)
```

---

## session

| Command | Usage | Notes |
|---|---|---|
| `session ls` | `xscore session ls` | One row per session: id, name, backend, and how many windows / panes / links it owns. |
| `session new` | `xscore session new [OPTIONS] <NAME> [URL]` | Creates a **native** session by default, always with its first link, window and pane. The URL says what that link is and how its pane looks; omit it for a shell on `$SHELL`. |
| `session adopt` | `xscore session adopt [OPTIONS] <NAME>` | Takes over a tmux session that already exists. The tmux name is used as the xscore session name. |
| `session show` | `xscore session show <NAME>` | Prints the backend and the ids of every link / window / pane it owns. |
| `session kill` | `xscore session kill <NAME>` | Destroys the session and everything in it — the only cascade in the model. For a tmux session this runs `kill-session`. |
| `session forget` | `xscore session forget <NAME>` | tmux only: drops xscore's rows and detaches, leaving the tmux session running. Refused for native sessions (the daemon owns those — use `kill`). |
| `session rename` | `xscore session rename <OLD> <NEW>` | Rejects a name already in use. |

### `session new` options

The first link is described by **one URL**, so there are only two flags left:

| Flag | Default | Meaning |
|---|---|---|
| `-c, --config <CONFIG>` | – | Use a saved link template (`link config add`). A URL given as well overrides the template field by field. |
| `--tmux` | off | Make this a tmux session; xscore creates it and drives it through one `tmux -C` client. The tmux session is named after the xscore session. **With this flag the URL names the tmux host**, not a pipe. |
| `--socket <NAME>` | `$XSCORE_SOCKET`, then the default server | tmux server socket (`tmux -L <NAME>`). An explicit flag wins over the environment. |

### URL grammar

```
shell://<command>[?cwd=…&cols=…&rows=…&name=…]
serial://<device>?baud=<rate>[&data_bits=…&parity=…&stop_bits=…&cols=…&rows=…]
ssh://<user>@<host>[:port][?identity=…&proxy=…&cols=…&rows=…]
```

With `--tmux` the positional is one of:

```
(omitted)                      the local default server
shell://[?cmd=…&cols=…]        the local server, said explicitly
ssh://[user@]host[:port][?identity=…&cmd=…&cwd=…&cols=…&rows=…]
?cmd=…&cols=…&rows=…           local server, options only
```

The **scheme is the link**: `shell://` means the control client runs locally,
`ssh://host` means it is `ssh … tmux -C attach`. That distinction is stored on
the control link, so `link list` shows `control/shell` or `control/ssh`, and
`link show` prints the corresponding address (`Shell { tmux … }` or
`Ssh { host, port, user, identity }`).

| Key | Applies to | Meaning |
|---|---|---|
| `cwd` | `shell://`, `--tmux` | Working directory of the first pane. Refused for serial/ssh links, which have no shell of their own. |
| `cols`, `rows` | everything | First pane size (default `80` × `24`). |
| `name` | pipe links | Link name. |
| `cmd` | `--tmux` | Command line for the first window, run through `sh -c`. |
| `identity`, `proxy` | `ssh://` | As `link add`. |
| `baud`, `data_bits`, `parity`, `stop_bits` | `serial://` | As `link add`. |

Unknown keys are an error, not ignored — a typo like `col=120` would otherwise
silently give you 80 columns.

### `session adopt` options

`--socket <NAME>` and `--ssh <DEST>`, with the same meaning as above.

```bash
xscore session new dev                                  # $SHELL at 80x24
xscore session new dev 'shell:///bin/bash?cwd=/var/log&cols=120&rows=40'
xscore session new hw  'serial:///dev/ttyUSB0?baud=115200&cols=100&rows=30'
xscore session new dev -c local                         # from a saved template

xscore session new work --tmux                          # local tmux server
xscore session new work --tmux 'shell://'               # same, said explicitly
xscore session new work --tmux 'shell://?cmd=htop'      # local, first window runs htop
xscore session new work --tmux 'ssh://box?cols=120&rows=40'
xscore session new work --tmux 'ssh://dev@box:2222?identity=~/.ssh/id_ed25519'

xscore session adopt build --socket scratch             # take over an existing tmux session
```

---

## link

A link is **one connection**. `link add` creates a *pipe* link (shell / ssh /
serial). A tmux session's control link is created with the session, not with
`link add` — `link add` inside a tmux session is refused.

| Command | Usage | Notes |
|---|---|---|
| `link add` | `xscore link add [OPTIONS] [URL]` | Creates the link plus its first window and pane. Omit the URL when using `-c`. |
| `link del` | `xscore link del --id <ID>` | Pipe links only: closes the transport and orphans every attached pane (their buffers are kept). Refused for a session's control link — use `session kill` / `session forget`. |
| `link show` | `xscore link show --id <ID>` | `LinkInfo` as JSON, including `connected`. |
| `link list` | `xscore link list [--session <NAME>]` | Columns: id, name, kind — `control/shell`, `control/ssh` for a session's control client, `pipe/shell`, `pipe/serial`, `pipe/ssh` for a pipe link. |
| `link config add` | `xscore link config add <NAME> <URL>` | Saves a URL template under `~/.config/xscore/links/`. |
| `link config remove` | `xscore link config remove <NAME>` | |
| `link config show` | `xscore link config show <NAME>` | |
| `link config list` | `xscore link config list` | |

### `link add` options

| Flag | Meaning |
|---|---|
| `-c, --config <CONFIG>` | Use a saved template instead of a URL. |
| `--id <UUID>` | Use a specific link id. Errors if it is already in use. |
| `--pane <PANE_ID>` | Bind an existing *orphan* pane instead of creating one. Refused if the pane is already attached, or if tmux owns it (`%N`). |
| `--session <NAME>` | Put the link in this session. Omitted: the only session if there is exactly one, else a session named `default` (created on demand). |

URL forms: `shell://` (local child in a pty), `serial://` (device, needs
`?baud=…`), `ssh://` (channel, key auth). tmux is **not** a URL scheme.

The same query parameters `session new` uses work here, because both parse one
grammar: `cwd` (shell only), `cols`, `rows`, `name`. An unknown key is an error.

```bash
xscore link add shell:///bin/bash --session dev
xscore link add 'serial:///dev/ttyUSB0?baud=115200' --session hw
xscore link config add logs 'ssh://deploy@host:22?identity=~/.ssh/id_ed25519'
xscore link add -c logs --session ops
```

---

## window

| Command | Usage | Notes |
|---|---|---|
| `window add` | `xscore window add [OPTIONS]` | In a tmux session this creates a real tmux window (`new-window`) and adopts `@N`; otherwise a native window. |
| `window list` | `xscore window list [--session <NAME>]` | Columns: id, name, session, cols, rows, tmux handle (`@N`, or `-` for native). |
| `window show` | `xscore window show --id <ID>` | `WindowInfo` as JSON. |
| `window del` | `xscore window del --id <ID>` | Removes the row; panes inside survive as orphans. For a tmux window this is `kill-window`. |
| `window add-pane` | `xscore window add-pane --window <WID> --pane <PID>` | Puts an existing pane in the window. |
| `window remove-pane` | `xscore window remove-pane --window <WID> --pane <PID>` | The pane stays alive as an orphan. |

### `window add` options

| Flag | Default | Meaning |
|---|---|---|
| `--name <NAME>` | `w-<first8 of id>`, or tmux's own name | Window name. |
| `--cols <COLS>` | `120` | Native windows only — tmux reports its own geometry. |
| `--rows <ROWS>` | `30` | Native windows only. |
| `--session <NAME>` | resolved default | Which session owns it. |

```bash
xscore window add --session work --name logs
xscore window list --session work
```

---

## pane

A pane is **a buffer with a viewport**. Its bytes come from its link: a pipe
link serves one pane, a tmux session's control link serves all of its panes
(addressed by `%N`).

| Command | Usage | Notes |
|---|---|---|
| `pane add` | `xscore pane add [OPTIONS]` | With `--window` pointing at a tmux window, this is a real `split-window`; otherwise an orphan or native pane. |
| `pane del` | `xscore pane del --id <ID>` | Full teardown: detaches, removes from the window, and kills the tmux pane if there is one. |
| `pane show` | `xscore pane show --id <ID>` | `PaneInfo` as JSON (`link_id`, `link_key`, geometry, `window_id`). |
| `pane list` | `xscore pane list [OPTIONS]` | Filters: `--session`, `--window`, `--link`. |
| `pane attach` | `xscore pane attach --pane <PID> --link <LID>` | **Pipe links only.** Creates the bridge; the link's previous pane is auto-detached (a pipe carries one reader). |
| `pane detach` | `xscore pane detach --pane <PID>` | **Pipe links only.** The pane survives as an orphan and keeps its buffer. |
| `pane resize` | `xscore pane resize --pane <PID> --cols <C> --rows <R>` | Geometry; forwards to the bridge when attached. Allowed on an orphan. |
| `pane move` | `xscore pane move --pane <PID> --x <X> --y <Y>` | Position only (pure UI metadata). |
| `pane add-to-window` | `xscore pane add-to-window --pane <PID> --window <WID>` | |
| `pane remove-from-window` | `xscore pane remove-from-window --pane <PID> --window <WID>` | |

### `pane add` options

| Flag | Default | Meaning |
|---|---|---|
| `--name <NAME>` | `p-<pane id>` | |
| `--cols` / `--rows` | `80` / `24` | Ignored when the window is a tmux window (tmux reports its own). |
| `--x` / `--y` | `0` / `0` | Position inside the window. |
| `--window <WID>` | – | Put the pane in this window. |
| `--session <NAME>` | resolved default | Which session owns it. |

```bash
xscore pane add --session work --window <wid>     # a tmux split
xscore pane list --session work
```

---

## data

| Command | Usage | Notes |
|---|---|---|
| `data read` | `xscore data read --pane <PID> [--bytes N]` | Reads the pane's buffer. `--bytes` (default **4096**) is how many of the **newest** bytes to copy. Non-destructive: reading twice returns the same bytes. Empty output means nothing arrived. |
| `data write` | `xscore data write --pane <PID> [--from-file <PATH>]` | Writes bytes to the pane. Input comes from stdin unless `--from-file` is given. |
| `data resize` | `xscore data resize --pane <PID> --cols <C> --rows <R>` | Resizes through the bridge. Errors for an orphan pane; use `pane resize` for geometry-only. |

Semantics worth knowing:

- **A newline is Enter.** `printf 'echo hi'` types the characters but a shell
  will not run them until `\n` arrives. `printf 'echo hi\n'` runs it.
- **`data read` returns terminal bytes, not clean text.** A pty with echo on
  shows what you typed, `\r` appears as `^M`, and escapes such as `\x1b[?2004h`
  are part of the stream. Filter them if you want plain output:
  `xscore data read --pane <pid> | sed 's/\x1b\[[0-9;?]*[a-zA-Z]//g' | tr -d '\r'`.
- For a tmux pane the payload must be UTF-8 and cannot contain `\0` (the control
  protocol is line-framed). Payloads with newlines are sent as `send-keys -H`.

```bash
pid=$(xscore session show dev | awk '/^pane:/{print $NF}' | head -1)
printf 'echo hello\n' | xscore data write --pane "$pid"
sleep 0.3
xscore data read --pane "$pid"
```

---

## daemon

The daemon owns every live connection: pipe transports (pty / serial / ssh) and
tmux control clients, plus both kinds of pane buffer.

| Command | Usage | Notes |
|---|---|---|
| `daemon serve` | `xscore daemon serve` | Foreground daemon: stays up until `daemon stop`, Ctrl-C or SIGTERM, and cleans up its socket on the way out. The copy that auto-start spawns adds `--idle-exit`, so it also leaves after 5 s with nothing to supervise; a bare `serve` deliberately does not. |
| `daemon status` | `xscore daemon status` | JSON on stdout (`pid`, `links`, `panes`, `tmux[]` with name / alive / panes / buffered), plus a human line per tmux session on stderr. Exits non-zero when no daemon is running. |
| `daemon stop` | `xscore daemon stop` | Closes transports and control clients and removes the socket. Buffers are memory, so they go; `state.json` and the tmux server survive. |

The daemon auto-starts on demand and exits after 5 s idle **only when it holds
no links and no control clients** — a managed tmux session keeps it alive.

---

## state

| Command | Usage | Notes |
|---|---|---|
| `state migrate` | `xscore state migrate` | In-place upgrade of `~/.config/xscore/state/state.json`. A v2 file gains the session tier (schema v3); a v3 file written by the old tmux path has those sessions converted to control-mode ones. Idempotent. |

An un-migrated file is refused at load with a message naming this command.

---

## config

`xscore config` is a stub: it prints a message and exits non-zero. AppConfig is
not implemented in v0.6.0.

---

## Environment

| Variable | Used for |
|---|---|
| `XSCORE_SOCKET` | Default tmux server name (`tmux -L`) for `session new --tmux` / `session adopt`. `--socket` wins. Tests use it for isolation. |
| `HOME`, `XDG_CONFIG_HOME` | Where state (`…/xscore/state/state.json`) and link templates (`…/xscore/links/`) live. |
| `XDG_RUNTIME_DIR` | Where the daemon socket lives, when set; otherwise under `$HOME/.config/xscore/run/`. |
| `PATH` | Must contain `tmux` (3.0+) for tmux sessions, and `ssh` for `--ssh`. |

---

## Common errors

Exact messages, so `grep`-ing a log works:

| Message | Meaning |
|---|---|
| `session not found: <name>` | No session by that name; `session ls` shows what exists. |
| ``a session named `<name>` already exists`` | Pick another name, or `session rename`. |
| ``a tmux session named `<name>` already exists; use `xscore session adopt <name>` to manage it`` | `session new --tmux` found the name taken on that socket. |
| ``session <id> is native — this process owns it; use `session kill` `` | `session forget` is tmux-only. |
| ``pane <pid> is not attached to any link (orphan — attach via `xscore pane attach --pane <pid> --link <lid>`)`` | Nothing to read or write yet. |
| ``pane <pid> is already attached to link <lid>; detach it first`` | A pane has one pipe link at a time. |
| ``link <lid> is not connected (its transport died with the previous process); re-add the link to reconnect`` | The daemon no longer holds this pipe link. |
| ``link kind `<kind>` has no window size; resize only applies to a pty, an ssh channel or a tmux pane`` | `data resize` on a serial link, or on a hydrated link with no live transport. |
| ``pane <pid> is owned by tmux and cannot be detached; use `pane del` instead`` | tmux panes cannot be unbound. |
| ``link <lid> is a tmux control link; use `session kill` or `session forget` instead`` | `link del` refuses the session's control client. |
| `session <id> is tmux-backed and already has its control link; add panes/windows, not links` | `link add` inside a tmux session. |
| `pane <pane> is not in window <window>` | `window remove-pane` / `pane remove-from-window` on a pane that is elsewhere. |
| `state.json schema_version=…` | Run `xscore state migrate` once. |

---

## Recipes

**A native session, then read and write its pane**

```bash
out=$(xscore session new dev 'shell:///bin/bash'); echo "$out"
pid=$(printf '%s\n' "$out" | awk '/^pane:/{print $NF}')

printf 'pwd; echo "it works"\n' | xscore data write --pane "$pid"
sleep 0.3
xscore data read --pane "$pid"

xscore session kill dev
```

**A tmux session that mirrors what tmux shows**

```bash
xscore session new work --tmux '?cols=100&rows=30'
tmux new-window -t work                          # outside xscore
xscore window list --session work                # two rows, @0 and @1
xscore session show work                         # ids for both windows/panes
xscore session kill work                         # takes the tmux session down
```

**Take over a session someone else started**

```bash
tmux new-session -d -s build
xscore session adopt build
xscore pane list --session build
```

**Clean up without killing the tmux session**

```bash
xscore session forget work    # xscore's rows go, tmux keeps running
```
