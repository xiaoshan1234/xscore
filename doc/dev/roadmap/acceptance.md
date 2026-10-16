# Acceptance Criteria

- `cargo build -p xscore` → success
- `cargo test -p xscore` → all tests pass (unit + integration)
- `xscore --help` lists `connect`, `link`, `pane`, `data`, `config`
- `xscore connect shell:///bin/bash` creates a link + pane; stdout prints both ids
- `xscore connect <same url>` (no `--link=existing`) creates a **second** link (default = no dedup)
- `xscore connect <shell url> --link=existing` returns `Error::LinkExistingUnsupported("shell")`
- `xscore link list/show/close/save/load` over `LinkInfo`
- `xscore pane list/show/close` over `PaneInfo`
- `xscore data read/write/resize` over pane-id (NOT link-id)
- `xscore config show/path/set/unset` operates on `~/.config/xscore/config.json`
- Address-keyed dedup: `LinkAddress` Hash impl + `LinkRegistry::find_by_address` is the single source of truth for "is this a new link or attach"

---

## What's deferred (explicitly NOT in this slice)

- Real russh connect + pty channel (Task 8 skeleton; SSH-integration phase)
- Real serial port open (Task 8 skeleton; serial-integration phase)
- `Sink::attach()` for "remote" mode in TmuxSink (current no-op; will wire when SshTransport gets real IO)
- Live streaming subscribe (`PaneBackend::snapshot` only for now)
- Tmux per-char send-keys perf optimization
- Cross-process manager state (CLI = in-memory; MCP server = daemon later)
- MCP server, IPC server, web UI — land in subsequent slices once this surface stabilizes
- RawSink full implementation (`?tmux=off` today is a no-op; tracked in open question)
- `ssh + --link=existing` (returns no-match today; defer to SSH-integration phase)

---

## Open questions (will be revisited, do not block the slice)

1. **Link uniqueness across CLI invocations.** Today `LinkRegistry` is in-memory; restarting `xscore` forgets all links. Real dedup across runs requires a persistence layer (likely a daemon). For this slice, address-keyed dedup is **per-process**.
2. **`?tmux=off` semantics.** RawSink is a no-op stub for capture/write. Future PR will wire it to a passthrough mode.
3. **TmuxSink "auto" mode.** Resolves to `local` for shell/serial, `remote` for ssh. Future: explicit `tmux_socket` query param to override.
4. **Pane persistence.** Closing the CLI remounts the pane from tmux (via `list-panes`) — for recovery, but in this slice the CLI is in-memory only.

---

## Execution Handoff

This plan is split across files in `doc/dev/roadmap/`:

- `overview.md` — architecture, modules, core traits
- `task-NN-*.md` — one file per task (sequential edits only need to read that one file)
- `acceptance.md` — this file

Next step: execute via `subagent-driven-development` skill — fresh subagent per task, two-stage review (spec compliance then code quality). Each task ends with `cargo test -p xscore` + `cargo build -p xscore` before commit.

Want me to proceed with execution, or revise the slice first?