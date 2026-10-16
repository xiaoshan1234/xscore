# Acceptance Criteria

- `cargo build -p xscore` → success
- `cargo test -p xscore` → all tests pass (unit + integration)
- `xscore --help` lists `link`, `pane`, `data`, `config`
- `xscore link add shell:///bin/bash` creates a link + pane; stdout prints both ids
- `xscore link add <same url>` (no `--link=existing`) creates a **second** link (default = no dedup)
- `xscore link add <shell url> --link=existing` returns `Error::LinkExistingUnsupported("shell")`
- `xscore link add ... -a <alias>` registers the link under `LinkRegistry.by_alias` for the current process lifetime
- `xscore link config add <alias> <url>` saves a template to `~/.config/xscore/links/<alias>.json`
- `xscore link config list/show/remove` operates on the templates directory
- `xscore link add -c <alias> <url>` reads the saved template and overrides the transport detail from the CLI URL
- `xscore link list/show/close/remove/open` operates on in-memory links (open is a re-attach, remove is unregister-only, close kills the transport + sink + every pane)
- `xscore pane add <link-id> [--name=...]` opens a new pane inside a live link
- `xscore pane list/show/close/remove/open` operates on in-memory panes (open is a re-attach, remove is unregister-only, close kills the tmux window)
- `xscore data read/write/resize` over pane-id (NOT link-id)
- `xscore config show/path/set/unset` operates on `~/.config/xscore/config.json`; `path` prints all 3 dirs (config, links, state)
- Address-keyed dedup: `LinkAddress` Hash impl + `LinkRegistry::find_by_address` is the single source of truth for "is this a new link or attach"
- Alias map: `LinkRegistry.by_alias` populated for `add_link -a`, queried by future alias-aware commands

---

## What's deferred (explicitly NOT in this slice)

- Real russh connect + pty channel (Task 8 skeleton; SSH-integration phase)
- Real serial port open (Task 8 skeleton; serial-integration phase)
- `Sink::attach()` for "remote" mode in TmuxSink (current no-op; will wire when SshTransport gets real IO)
- Live streaming subscribe (`PaneBackend::snapshot` only for now)
- Tmux per-char send-keys perf optimization
- Cross-process manager state (CLI = in-memory; MCP server = daemon later). `state/links/<id>.json` is reserved but unused this slice.
- MCP server, IPC server, web UI — land in subsequent slices once this surface stabilizes
- RawSink full implementation (`?tmux=off` today is a no-op; tracked in open question)
- `ssh + --link=existing` (returns no-match today; defer to SSH-integration phase)
- `link open <link-id>` and `pane open <pane-id>` are stubs (`todo!()`) — full re-attach implementation lands in attach-persistence PR. This slice makes `add`/`close`/`remove` work end-to-end.

---

## Open questions (will be revisited, do not block the slice)

1. **Link uniqueness across CLI invocations.** Today `LinkRegistry` is in-memory; restarting `xscore` forgets all links + their panes. Real dedup across runs requires a persistence layer (likely a daemon). For this slice, address-keyed dedup is **per-process**. `link open <link-id>` would address this — see "deferred" list.
3. **`?tmux=off` semantics.** RawSink is a no-op stub for capture/write. Future PR will wire it to a passthrough mode.
4. **TmuxSink "auto" mode.** Resolves to `local` for shell/serial, `remote` for ssh. Future: explicit `tmux_socket` query param to override.
5. **Pane persistence.** Closing the CLI forgets the in-memory pane registry; `tmux -L xscore` windows keep running but are invisible to `pane list`. `pane open <pane-id>` (when link is also attached) would address this — see "deferred" list.
6. **Alias uniqueness.** Two `link add ... -a web` calls produce different link_ids but the same `by_alias` key, so the second overwrites the first in `by_alias`. Acceptable for a runtime-only map.

---

## Execution Handoff

This plan is split across files in `doc/dev/roadmap/`:

- `overview.md` — architecture, modules, core traits, CLI surface
- `task-NN-*.md` — one file per task (sequential edits only need to read that one file)
- `acceptance.md` — this file

Adjacent directories (also tracked in git):

- `../adr/0001-link-pane-model.md` through `0004-link-existing-dedup.md` — design rationale
- `../architecture/diagrams/*.mmd` — Link×Pane topology, lifecycle, module-deps (parse-validated against mermaid@9)

Next step: execute via `subagent-driven-development` skill — fresh subagent per task, two-stage review (spec compliance then code quality). Each task ends with `cargo test -p xscore` + `cargo build -p xscore` before commit.

Want me to proceed with execution, or revise the slice first?