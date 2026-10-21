# v0.6.0 — the session tier, and tmux as a stream

Two things change shape in this release, and one of them is breaking.

## 1. `Session` is the workspace

```
Session ── a workspace, and the backend that keeps it alive
  ├ Window ── a layout container
  ├ Pane   ── a buffer with a viewport
  └ Link   ── one connection
```

"Does this go through tmux?" used to be a property of a *link*
(`?tmux=on|auto|local|remote`). It was the wrong place for it: a link is a
connection, and whether a connection is multiplexed is a property of the
workspace it lives in. `SessionBackend::{Native, Tmux{host,socket,session}}`
now carries that, and every link, window and pane carries its `session_id`.

`session kill` is the only cascade in the model. `session forget` drops
xscore's rows for a tmux session and leaves tmux running.

## 2. tmux is driven through control mode

`tmux -C attach` is now the session's single link. Every pane's bytes arrive as
`%output %N` on that one connection, structure arrives as notifications, and a
write is one `send-keys` line — no shell, no `argv`, no `ARG_MAX`.

**A pane is a stream, not a screen.** Before, reading a tmux pane meant
`capture-pane`: a rendered grid, with no ordering and no notion of "bytes since
I last looked". Now `data read` returns the bytes the pane produced.

Things that were learned by pointing the client at a real tmux 3.5a, and are
now in the code and tests:

- `-CC` **does not work with pipes.** It disables echo via `tcgetattr` on the
  client's terminal and dies with `tcgetattr failed: Inappropriate ioctl for
  device`. Use `-C`; `-CC` is for clients that own a tty (iTerm2 allocates a
  pty).
- **tmux numbers its own replies.** `%begin <ts> <num> <flags>` — the `num` is
  tmux's counter, not yours, so replies must be matched by order. Waiting for
  your own numbering hangs forever.
- **Pane output can begin with `%`.** `list-panes -F '#{pane_id}'` returns `%0`,
  which a prefix-based router would mistake for a notification.
- **A raw newline ends a tmux command.** Payloads containing `\n` go through
  `send-keys -H <hex>`; `\0` is impossible on a line protocol.
- **Nothing is replayed.** A pane discovered late has no history.

## 3. The daemon owns every live connection

ADR 0009 said the daemon owns the live I/O; until now that meant native
transports. It now also owns tmux control clients and both kinds of pane buffer,
which is what makes `data read` a stream *across CLI processes* rather than a
snapshot:

```
$ xscore data write --pane <pid>        # process A: XYZ, no newline
$ xscore data read  --pane <pid>        # process B
XYZ
```

`daemon status` shows each managed tmux session (`alive`, attached panes,
buffered bytes). Consequences: the daemon does not idle-exit while it holds a
control client, and `daemon stop` closes them (buffers are memory, so they go;
`state.json` and tmux survive).

## Breaking changes

| removed | replaced by |
|---|---|
| `?tmux=on\|auto\|local\|remote` | `session new --tmux` / `session adopt` |
| `link open` / `link close` | `session attach` / `session kill` / `session forget` |
| `link sync [--heal]` | structure is read from tmux on touch |
| `window set-layout`, `window add --link`, `window add --layout` | `window add --session` |
| `link add --pane` inside a tmux session | tmux creates its own panes |
| `WindowLayout` (`single`/`tabs`/`split_h`/`split_v`/`grid`) | tmux owns layout |
| `SinkConfig`, `Sink`, `RawSink`, `TmuxSink`, `PaneInfo.sink_handle` | `EndpointRef::{Pipe, Control}`, `PaneInfo.link_key` |

## Upgrading

`state.json` is schema v3. There are two migrations and `state migrate` runs
whichever applies:

- **v2 → v3**: adds the session tier, one session per link.
- **v3 with old tmux links** (written by a pre-0.6 build): converts each such
  session into a control-mode session pointing at the same tmux session name,
  and drops its window/pane rows — they are re-enumerated when the session is
  next attached. Native sessions are untouched.

An un-migrated file is refused with a message naming the command, rather than
being loaded into a half-empty state.

## Also in this release

- Shell links are **real ptys**: `TERM` is set, `isatty` is true, echo and job
  control work, and `data resize` reaches `TIOCSWINSZ` (it used to return `Ok`
  while doing nothing; a kind with no window size now errors instead).
- `window list` / `pane list` / `link list` take `--session`.
- A `$HOME` that no longer exists no longer breaks every pty spawn with a
  misleading `No such file or directory`.

## Known limits

- Pane buffers are daemon memory (`daemon stop` loses them).
- A tmux pane's `data write` needs UTF-8; payloads with newlines use the hex
  form internally.
- Remote tmux uses the daemon's ssh environment, so a key added after the daemon
  started is not seen until it restarts.
- Concurrent mutating CLI calls still last-writer-wins on `state.json`.
