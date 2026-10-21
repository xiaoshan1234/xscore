# xscore v0.5.0 — release notes

**Tag:** `v0.5.0`
**Date:** 2026-10-20
**Test count:** 93 (54 unit + 29 cli_smoke + 10 data_smoke), 0 failed, 0 ignored
**Warnings:** 0

> **No breaking changes vs v0.4.1.** `state.json` stays `schema_version: 2`;
> no migration, no flag changes.
>
> The one behaviour change worth knowing: `link show`'s `connected` is now
> a live probe (it can flip to `false` when the daemon stops and back to
> `true` when something re-attaches), and `data read` on a *detached*
> direct pane returns its buffer instead of `NotAttached`.

## The problem this release fixes

In v0.4.x this was impossible, for every user, always:

```bash
$ xscore link add shell:///bin/cat
link: ...  window: ...  pane: ...
$ xscore data write --pane <pid>
Error: link ... is not connected (its transport died with the previous process)
```

The transport — the `/bin/cat` child, a serial fd, an ssh channel — was
created inside the CLI process, and that process exits. v0.4.1 made the
failure honest (`connected: false` + `LinkNotConnected`) but not usable.
The pane's ring buffer had the same problem: it lived and died with the
command.

tmux links never suffered from this, and for an obvious reason: **tmux is
a resident server**. v0.5.0 gives direct links the same property.

## What's new

### A resident daemon

```bash
$ xscore link add shell:///bin/cat      # auto-starts the daemon
link: ...  window: ...  pane: ...

$ xscore data write --pane <pid> <<< 'hello'   # different process
$ xscore data read --pane <pid>                # yet another one
hello
```

`xscore daemon serve` holds exactly two things:

```
links: link_id → Arc<dyn Transport>        (shell / serial / ssh)
panes: pane_id → { link, RingBuffer, pump }   (the pump feeds the buffer)
```

It is a byte-level cache with a lifetime. The CLI keeps the **metadata**
(`state.json`: ids, bindings, geometry) and remains the only writer of
that file. Killing the daemon is therefore a reconnect, never data loss.

### `daemon` verbs

```bash
$ xscore daemon status
{
  "ok": true,
  "status": {
    "pid": 2501192,
    "links": ["01a15c20-6ae1-7014-a259-93eec3efbe4b"],
    "panes": [
      { "pane": "01a15c20-...-74a4f4bed4a0",
        "link": "01a15c20-...-6ae1-7014-a259-93eec3efbe4b",
        "buffered": 6 }
    ]
  }
}

$ xscore daemon stop
daemon stopped
$ xscore link show --id <lid> | grep connected
  "connected": false,
```

Lifecycle rules:

| rule | value |
|---|---|
| socket | `$XDG_RUNTIME_DIR/xscore/daemon.sock`, else `$HOME/.config/xscore/run/daemon.sock` |
| start | auto, on the first command that needs a transport (2 s budget) |
| idle exit | no links **and** 5 s without a request |
| explicit stop | `xscore daemon stop` (closes transports, removes socket + lock) |
| single instance | `daemon.lock` taken with `create_new` before binding; stale lock reclaimed |
| stale socket | connect; if nobody answers, unlink and rebind |

### One daemon, not one per link

A pane may switch links (`pane attach --pane P --link B`), and its buffer
must follow it. Buffers can only follow panes inside one process, so the
daemon is global and re-binding is a pointer change plus a new pump — no
copying across processes, no bytes lost or duplicated.

The Q1 rule (one active pane per direct link) is enforced inside the
daemon too: two pumps on one transport would double every byte into two
buffers.

### Protocol

Unix socket, `[u32 little-endian length][JSON UTF-8]`, ops:
`ping`, `status`, `create_link`, `delete_link`, `create_pane`,
`delete_pane`, `attach`, `detach`, `read`, `write`, `resize`, `stop`.
Binary payloads are hex strings — no new dependencies, and the wire stays
readable with `socat`. A `read` is a non-destructive snapshot of the
pane's 64 KiB scrollback and waits up to 200 ms for bytes, exactly like
the in-process bridge did.

## Fixed along the way

- **`state.json` temp-file race.** Two concurrent CLI runs both wrote
  `state.json.tmp`; the loser's `rename` failed with ENOENT. The temp
  name is now per-process.
- **Two daemons on one socket.** Concurrent auto-starts could both
  unlink-and-bind, leaving one daemon listening on an orphaned inode with
  half the links. Startup is now serialised by a lock file.
- **Leaked transports on failed `link add`.** An attach/state-save error
  now tears the daemon link back down instead of leaving a transport (and
  a daemon that can never idle-exit) behind.

## Tests

The daemon exists to cross a process boundary, so the tests cross one:

- `xscore_daemon_direct_link_survives_process` — create in process A,
  write in process B, read in process C. This is the acceptance test.
- `xscore_daemon_status_lists_link`,
  `xscore_daemon_link_show_connected_true`,
  `xscore_daemon_stopped_link_not_connected`,
  `xscore_daemon_buffer_survives_detach_across_processes`.
- `xscore_direct_link_stays_connected_after_restart` — the v0.4.1
  `..._reports_not_connected_after_restart` scenario, inverted: the
  limitation it documented is gone.
- In-process unit tests drive the daemon's state machine against a
  scripted transport (attach/detach/delete/Q1, buffer retention, error
  text), plus framing, hex, and the `op` JSON shapes.
- `data_smoke` is unchanged and still exercises `DirectBridge` without a
  daemon — the in-process path stays supported.

Each CLI test gets its own `HOME`, hence its own socket, and stops its
daemon on the way out; the 5 s idle exit covers anything else.

## Known limits

- Live I/O does not survive a **daemon** restart. Metadata, bindings and
  `state.json` do; `pane attach` (or `link add`) re-creates the transport
  from the persisted `TransportConfig`. Scrollback is in-memory: it
  outlives CLI processes, not the daemon.
- `daemon stop` is all-or-nothing for direct links. tmux links are
  unaffected (tmux owns them).
- Two concurrent mutating CLI calls still don't serialise their
  `state.json` writes (last writer wins). The write itself no longer
  races, and both calls now share one daemon.
- `link open` (tmux) and `xscore config` remain stubs.

## Upgrade

Nothing to do: same state schema, same commands. `cargo install --path .`
(or `cargo build --release`). The daemon is not a service you install —
the CLI starts and stops it as needed.
