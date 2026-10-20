# xscore v0.4.0 — release notes

**Tag:** `v0.4.0`
**Date:** 2026-10-20
**Test count:** 62 (29 unit + 23 cli_smoke + 10 data_smoke), 0 failed, 0 ignored
**Warnings:** 0

> **⚠️ Breaking changes vs v0.3.0**
>
> 1. `xscore link add <url>` now creates a window **and** a pane, and
>    prints three ids.
> 2. `state.json` is now `schema_version: 2`. v0.3.0 (and older) files
>    are **rejected** with an actionable error. There is no migration:
>    `rm ~/.config/xscore/state/state.json` and re-add your links.
>
> Everything else is additive.

## What's new in v0.4.0

### `link add` gives you something you can type into

```bash
$ xscore link add shell:///bin/bash
link:   01928e0a-7d10-7000-8000-000000000001
window: 01928e0a-7d10-7000-8000-000000000002
pane:   01928e0a-7d10-7000-8000-000000000003

$ echo 'echo hi' | xscore data write --pane <pid>
$ xscore data read --pane <pid>
hi
```

Three branches, all explicit:

| Command | Result |
|---|---|
| `link add <url>` | virtual window (120×30) + pane (80×24) with a ring buffer |
| `link add '<url>?tmux=on'` | adopts tmux's `@N` window and `%N` pane, with tmux's geometry |
| `link add <url> --pane <pid>` | binds that existing pane (direct links only) |

`--pane` is refused for tmux links (`TmuxLinkCannotBindPane`) — tmux owns
its panes — and refused for a pane that is already attached
(`PaneAlreadyAttached <link>`), so a pane is never silently stolen from
another link.

### tmux adoption: `%N` / `@N` are the identity anchors

`PaneInfo.sink_handle` (`"%3"`) and `WindowInfo.tmux_window_handle`
(`"@0"`) record what tmux actually created. Indexes are display data and
are never used for identity. Consequences:

- `window add --link <tmux-lid>` creates a real tmux window and adopts it.
- `pane add --window <wid>` splits inside that specific window (no
  implicit window 0) and adopts the new pane's handle + geometry.
- Hydration rebuilds tmux bridges from `%N` alone: `data read/write` work
  after a process restart for tmux panes.

### `link sync` — registry vs tmux

```bash
$ xscore link sync --id <lid>
drift: pane %1 exists in tmux but not in xscore (not adopted)
drift: pane %2 in xscore is gone from tmux
run `xscore link sync --id <lid> --heal` to apply

$ xscore link sync --id <lid> --heal
healed: 1 row(s) corrected, 1 removed
```

`--heal` corrects geometry/name/layout from tmux and drops registry rows
whose tmux object is gone. Panes you created directly in tmux are
**reported only** — adoption is a deliberate act, never a side effect.

### Pane buffers are real, and detach does not kill the link

`DirectBridge` now owns a 64 KiB `RingBuffer` fed by a background reader
task; `data read` returns a snapshot of it (tmux `capture-pane`
semantics — reading twice does not consume the screen). The buffer is
kept per `PaneId`, so it survives `pane detach` and a later re-attach.

`DirectBridge::close()` stops only its own pump. The link's transport is
closed exactly once, in `link del`. In v0.3.0, detaching one pane killed
the link's transport and left a dead link in the registry.

## Fixed (the P0 list from the v0.3.0 audit)

| ID | Bug | Fix |
|---|---|---|
| P0-a | `detach_pane` → `transport.close()` killed the link | `close()` stops the per-pane pump only; transport closes in `del_link` |
| P0-b | `DirectBridge` had no buffer (`read` = pass-through) | ring buffer + background pump, per-pane, survives detach |
| P0-c | `PaneId` had no mapping to tmux `%N` | `PaneInfo.sink_handle` |
| P0-d | `WindowId` had no mapping to tmux `@N` | `WindowInfo.tmux_window_handle` |
| P0-e | `state.json` silently parsed into an empty registry | `schema_version: 2` gate with an explicit error |

Plus: the pump polls `transport.read()` under a 10 ms timeout, because
transports may serialize read/write behind one lock (`ShellTransport`
does) — a blocking reader would starve `data write` forever.

## Semantics worth re-reading

- `pane detach` on a tmux pane → `TmuxPaneNotDetachable`. Use `pane del`
  (kills the real tmux pane).
- `link del` on a tmux link → `kill-session`; adopted windows/panes are
  removed with it.
- `link del` on a direct link → panes become orphans (`link_id: null`)
  and keep their buffers; the transport is closed once.
- `pane del` on a tmux pane → `kill-pane`.

## Upgrade checklist

```bash
# 1. Upgrade the binary.
cargo install --path .

# 2. Reset the state file (schema v2 has no migration from v0.3.0).
rm ~/.config/xscore/state/state.json

# 3. (Optional) clear stale tmux sessions from v0.3.0 experiments.
tmux -L xscore ls
```

If you skip step 2, every command fails fast with:

```
Error: config error: state.json schema_version=0, expected 2;
delete ~/.config/xscore/state/state.json to reset
```

## Tests

```bash
cargo test -p xscore -- --test-threads=1
# 62 tests: 29 unit + 23 cli_smoke + 10 data_smoke, 0 failed
```

The CLI tests each get their own `HOME` and `XSCORE_SOCKET`, so they
never touch your real state file or your tmux server.

## Known limitations

- `link open` for tmux is still a `todo!()` stub (v0.5.0).
- Direct panes cannot keep live I/O across a process restart (their
  transport died with the process); tmux panes can.
- `ssh://` / `serial://` remain implemented-but-untested (need real
  hardware / server).
