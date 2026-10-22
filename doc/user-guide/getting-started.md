# Getting started

A hands-on walk through xscore, from an empty directory to a session you can
read and write in two tabs. Everything here was run against the v0.6.0 build;
copy-paste works.

If you only want the flags, [`commands.md`](commands.md) is the reference.
This file is the "why it is shaped like that, and what do I type" version.

---

## What you need

```bash
cargo build --release          # Rust 1.95+; the binary lands in target/release/xscore
./target/release/xscore --help
```

- **tmux 3.0+** on `PATH` — only for tmux-backed sessions.
- **`ssh` on `PATH`** — only for tmux sessions on another host (`--tmux 'ssh://box'`).
- Nothing else. No config file is required to start; `state.json` is created on
  first use.

Put it somewhere on your `PATH` if you want to type `xscore` instead of a path:

```bash
install -m755 target/release/xscore ~/.local/bin/xscore
```

---

## The four ideas, in one screen

```
Session   a workspace. It owns a backend: native (xscore's daemon keeps it) or
          tmux (the tmux server keeps it).
  Window  a layout container. In a tmux session it mirrors a tmux window (@N).
  Pane    a buffer with a viewport. This is what you read from and write to.
  Link    one connection: a pty, a serial device, an ssh channel — or the single
          control client that a tmux session uses to reach its server.
```

Two rules carry most of the weight:

1. **A pane's bytes come from its link.** A pipe link (shell/ssh/serial) serves
   exactly one pane, because two readers on one pty would steal each other's
   bytes. A tmux session's control link serves all of its panes, because tmux
   says which pane each chunk belongs to (`%output %N`).
2. **The CLI is one process per command; the daemon owns anything live.** That
   is why you can write in one terminal and read in the next.

---

## 1. Your first session

```console
$ xscore session new dev 'shell:///bin/bash'
session: 01a15e7f-eaa8-707f-a23b-59da7c89a6dc
link:    01a15e7f-eaaa-7545-ba94-71587c3ddbe1
window:  01a15e7f-eac2-7071-9999-7b77e17c4fb0
pane:    01a15e7f-eac2-7071-9999-7b78ad952378
```

One command, four ids. The session is **native**: xscore started `/bin/bash` on
a real pty and handed it to its daemon, which owns the pty from now on. The
shell keeps running when this command exits — that is the whole point.

`session show` prints the ids again whenever you lose them (they are UUIDv7, so
they are also sortable by creation time):

```console
$ xscore session show dev
session: 01a15e7f-…
name:    dev
backend: native
created: 2026-10-21T12:46:43.880125708+00:00
link:    01a15e7f-…
window:  01a15e7f-…
pane:    01a15e7f-…
```

And `session ls` gives you the overview:

```console
$ xscore session ls
SESSION ID                            NAME     BACKEND  WINDOWS  PANES  LINKS
01a15e7f-eaa8-707f-a23b-59da7c89a6dc  dev      native   1        1      1
```

The URL is how you say what the first link is. A few shapes, then the two
sections that follow use them:

```bash
xscore session new dev 'shell:///bin/bash?cwd=/var/log&cols=120&rows=40'
xscore session new hw  'serial:///dev/ttyUSB0?baud=115200'
```

Every session always has a link, a window and a pane — there is no
empty-workspace form. To get one later, delete its children (`pane del`,
`link del`, `window del`); the workspace stays.

A saved template replaces the URL: `xscore session new dev -c local`.

---

## 2. Write to the pane, then read it back

Writing is one command. The payload comes from stdin (or `--from-file`):

```console
$ printf 'echo hello from xscore\n' | xscore data write --pane 01a15e7f-…a378
wrote 23 bytes
```

Reading is another:

```console
$ xscore data read --pane 01a15e7f-…a378
^[[?2004hloner@WinLite:~$ echo hello from xscore^M
^[[?2004l^Mhello from xscore^M
^[[?2004hloner@WinLite:~$
```

Three things to notice, because they explain most surprises:

- **`\n` is Enter.** Without it, the bytes sit in the terminal as typed text and
  nothing runs. `printf 'ls\n'` runs `ls`; `printf 'ls'` does not.
- **The output is raw terminal bytes.** `^M` is `\r`, and `^[[?2004h` is the
  bracketed-paste mode the shell turns on. Want clean text?
  `xscore data read --pane <p> | sed 's/\x1b\[[0-9;?]*[a-zA-Z]//g' | tr -d '\r'`.
- **`data read` is a snapshot of the buffer, not a queue.** It returns the
  newest 4096 bytes (`--bytes N` to change) and consumes nothing, so reading
  twice gives the same bytes. Empty output means nothing has arrived.

Control keys are just bytes — there is no key-name syntax:

```bash
printf '\003' | xscore data write --pane <p>   # Ctrl-C
printf '\004' | xscore data write --pane <p>   # Ctrl-D (EOF)
printf 'exit\n' | xscore data write --pane <p> # leave the shell
```

---

## 3. A second window, and a pane that switches links

Windows group panes; panes hold the buffers.

```console
$ xscore window add --session dev --name logs
window: 01a15e7f-ec80-737a-ba69-34a660fbf335

$ xscore pane add --session dev --window 01a15e7f-ec80-…f335 --cols 100 --rows 40
pane:   01a15e7f-ec8e-7377-9d7c-21f8e2f3df49
```

A pane created this way has **no link** — it is an orphan, a buffer with nothing
behind it yet. `pane list` shows that as `-` in the link column:

```console
$ xscore pane list --session dev
PANE ID      LINK ID      WINDOW ID    COLS  ROWS  POS  X  Y
01a15e7f-…   01a15e7f-…   01a15e7f-…   80    24    0x0  0  0
01a15e7f-…   -            01a15e7f-…   100   40    0x0  0  0
```

Attaching a different link to a pane is how you "move" a pane between
connections — a pane is a viewport, not a connection:

```console
$ xscore link add shell:///bin/bash --session dev
link:   01a15e7f-ec9b-720e-bfe3-9005dbbd60a5
window: 01a15e7f-ec9d-7301-8fe1-91e8ffe46a95
pane:   01a15e7f-ec9d-7301-8fe1-91e91e6da25d
```

`link add` also makes a window and a pane so the link is usable immediately.
Now give the *orphan* that link instead — the pane that was on it is detached
automatically, because a pipe has one reader:

```console
$ xscore pane attach --pane 01a15e7f-…df49 --link 01a15e7f-…60a5
attached: 01a15e7f-…df49 → link 01a15e7f-…60a5

$ xscore pane list --session dev
01a15e7f-…   01a15e7f-…   01a15e7f-…   80   24    0x0  0  0   # original pane, still on it
```

`pane detach` does the inverse and keeps the buffer:

```console
$ xscore pane detach --pane 01a15e7f-…df49
detached: 01a15e7f-…df49
```

---

## 4. A tmux session

Same four verbs, one flag:

```console
$ xscore session new work --tmux '?cols=100&rows=30'
session: 01a15e7f-ecdd-7101-94e6-cb782efae43b
link:    01a15e7f-ecde-74ad-a927-3a30733b3e3a
window:  01a15e7f-ece2-72b0-bca0-f485800462de
pane:    01a15e7f-ece2-72b0-bca0-f486ac6d6077
```

xscore ran `tmux new-session -d -s work`, then attached **one** `tmux -C`
client, which is the session's single link. Every pane's bytes arrive on that
one connection and are demultiplexed by `%output %N`.

`session ls` shows the backend, including where it lives:

```console
SESSION ID   NAME  BACKEND                   WINDOWS  PANES  LINKS
01a15e7f-…   dev   native                    3        3      2
01a15e7f-…   work  tmux local/default/work   1        1      1
```

Now touch tmux directly, outside xscore:

```bash
tmux new-window -t work
```

If the session lives on a named socket (`--socket scratch`, or `XSCORE_SOCKET`),
pass it through: `tmux -L scratch new-window -t work`.

The next `xscore window list` sees it, because structure is read from tmux on
demand rather than cached:

```console
$ xscore window list --session work
WINDOW ID    NAME   SESSION  COLS  ROWS  TMUX
01a15e7f-…   bash   work     100   30    @0
01a15e7f-…   bash   work     100   30    @1
```

Reading and writing a tmux pane uses the same commands as any other pane:

```bash
printf 'echo hi\n' | xscore data write --pane <tmux-pane-id>
xscore data read --pane <tmux-pane-id>
```

Two tmux-only limits: the payload must be **UTF-8** and cannot contain `\0`
(the control protocol is line-framed). Newlines are fine — they go through
`send-keys -H`.

**Ending a tmux session** — the one place where the verb matters:

| | effect on tmux |
|---|---|
| `xscore session kill work` | runs `kill-session`; the tmux session is gone |
| `xscore session forget work` | xscore stops managing it; **tmux keeps running** |

**Taking over a tmux session that already exists:**

```console
$ tmux new-session -d -s build
$ xscore session adopt build
session: 01a15e7f-ab09-…
link:    01a15e7f-ab0a-…
window:  01a15e7f-ab0f-…
pane:    01a15e7f-ab0f-…
```

---

## 5. Other connections

`link add` takes a URL. Three schemes:

```bash
xscore link add shell:///bin/bash                      # a local shell on a pty
xscore link add 'serial:///dev/ttyUSB0?baud=115200'    # a serial device
xscore link add 'ssh://me@box:22?identity=~/.ssh/id_ed25519'
```

Repeat a long URL without retyping it:

```bash
xscore link config add prod 'ssh://deploy@prod:22?identity=~/.ssh/id_ed25519'
xscore link add -c prod --session ops
xscore link config list
```

`ssh://` uses key auth through `russh`. For tmux over ssh, don't use a link at
all — `xscore session new work --tmux 'ssh://box'` execs the real `ssh`, so
`~/.ssh/config`, `ProxyJump` and your agent apply.

---

## 6. What the daemon is doing

```console
$ xscore daemon status
tmux 01a15e7f-…: work alive=true panes=2 buffered=0
{
  "ok": true,
  "status": {
    "pid": 3191989,
    "links": ["01a15e7f-…", "01a15e7f-…"],
    "panes": [
      { "pane": "01a15e7f-…", "link": "01a15e7f-…", "buffered": 102 }
    ],
    "tmux": [
      { "session": "01a15e7f-…", "name": "work", "alive": true, "panes": 2, "buffered": 0 }
    ]
  }
}
```

It starts by itself the first time a command needs a live connection, and the
copy it spawns leaves after 5 s with nothing to hold. `xscore daemon serve` run
by you is the opposite: it stays up until `daemon stop`, Ctrl-C or SIGTERM.

What `daemon stop` costs:

- pane **buffers** are memory — they are gone (they were scrollback, not state);
- pipe transports close: `link show` then reports `"connected": false`, and
  `pane attach` re-creates one;
- tmux sessions **survive**, because tmux was never xscore's process.

Metadata is unaffected either way: it lives in `state.json`.

---

## 7. Where your state lives

| What | Path |
|---|---|
| sessions / windows / panes / links | `~/.config/xscore/state/state.json` (or `$XDG_CONFIG_HOME/xscore/…`) |
| saved link templates | `~/.config/xscore/links/` |
| daemon socket | `$XDG_RUNTIME_DIR/xscore/daemon.sock`, else `~/.config/xscore/run/daemon.sock` |

```console
$ head -c 120 ~/.config/xscore/state/state.json
{
  "schema_version": 3,
  "sessions": [
    {
      "id": "01a15e80-…",
```

No bytes are ever stored there — only rows. If the file came from an older
build, every command refuses to load it and says what to do:

```console
$ xscore link list
Error: state.json schema_version=2, expected 3; run `xscore state migrate`

$ xscore state migrate
migrated state.json to schema v3: 2 session(s) created
```

`XSCORE_SOCKET` picks the tmux server (`tmux -L <name>`) for `session new
--tmux` / `session adopt`; it is also how you keep experiments away from your
real tmux server:

```bash
XSCORE_SOCKET=scratch xscore session new work --tmux
```

---

## 8. Everyday recipes

**Send a command and show what came back**

```bash
pid=$(xscore session show dev | awk '/^pane:/{print $NF}')
printf 'ls -l /etc/hosts\n' | xscore data write --pane "$pid"
sleep 0.3
xscore data read --pane "$pid" | cat -v
```

**Interrupt whatever is running**

```bash
printf '\003' | xscore data write --pane "$pid"
```

**Run a script in a pane**

```bash
xscore data write --pane "$pid" --from-file ./setup.sh
```

**Keep a serial device's output in a buffer you can inspect later**

```bash
lid=$(xscore link add 'serial:///dev/ttyUSB0?baud=115200' --session hw | awk '/^link:/{print $NF}')
pid=$(xscore pane list --link "$lid" | tail -n +2 | awk '{print $1}')
xscore data read --pane "$pid" --bytes 65536
```

**Clean up everything**

```bash
xscore session ls                       # names
xscore session kill <name>              # per session; the only cascade
xscore daemon stop                      # buffers go, metadata stays
```

---

## 9. When something goes wrong

| Message | What it means | Fix |
|---|---|---|
| `session not found: dev` | No such name. | `xscore session ls` |
| `pane … is not attached to any link` | The pane has no connection behind it. | `pane attach --pane <p> --link <l>`, or `link add … --pane <p>` |
| `link … is not connected (its transport died …)` | The daemon no longer holds that pipe link (restart, `daemon stop`). | `pane attach` or a new `link add` |
| `pane … is owned by tmux and cannot be detached` | tmux panes belong to tmux. | `pane del` |
| `link … is a tmux control link; use session kill or session forget` | You tried `link del` on the control client. | `session kill` / `session forget` |
| `a tmux pane takes UTF-8 text` | Binary payload into a tmux pane. | Use a native link for binary |
| `state.json schema_version=…` | Old file. | `xscore state migrate` |

Useful checks, in order:

```bash
xscore session ls              # do the rows exist?
xscore pane list --session dev # is the pane attached? (LINK ID != -)
xscore daemon status           # is anything live? which panes have bytes?
xscore daemon serve            # run it in the foreground to watch it work
```

---

## 10. Where to read next

- [`commands.md`](commands.md) — every command and flag.
- [`../dev/arch/arch.md`](../dev/arch/arch.md) — the model, and why the daemon
  owns what it owns.
- [`../dev/adr/0010-session-layer-and-control-mode.md`](../dev/adr/0010-session-layer-and-control-mode.md)
  — the decisions behind the session tier and control mode.
- `RELEASE-NOTES-v0.6.0.md` at the repo root — what changed, and what breaks
  when upgrading from 0.5.x.
