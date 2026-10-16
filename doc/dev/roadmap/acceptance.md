# Acceptance Criteria

- `cargo build -p xscore` → success
- `cargo test -p xscore` → all tests pass (unit + integration)
- `xscore --help` lists `link`, `pane`, `data`, `config`
- `xscore link add shell:///bin/bash` creates a link + pane; stdout prints both ids
- `xscore link add <same url>` creates a **second** link (always-new under ADR 0005)
- `xscore link add` with neither `<url>` nor `-c CONFIG` returns an error ("provide either <url> or -c CONFIG")
- `xscore link open <shell url>` returns `Error::OpenOnlyForTmux("shell")`
- `xscore link close <shell-link-id>` returns `Error::CloseOnlyForTmux("shell")`
- `xscore link config add <name> <url>` saves a template to `~/.config/xscore/links/<name>.json`
- `xscore link config list/show/remove` operates on the templates directory
- `xscore link add -c <name> <url>` reads the saved template and overrides the transport detail from the CLI URL
- `xscore link list/show/del` operates on in-memory links (`del` is the universal teardown verb)
- `xscore link open <tmux url>` attaches to an existing tmux session (returns `(link_id, pane_id)`)
- `xscore link close --id <link-id>` on a tmux link leaves the tmux session running on the server
- `xscore pane add <link-id> [--name=...]` opens a new pane inside a live link
- `xscore pane list/show/close/remove/open` operates on in-memory panes (open is a re-attach stub, remove is unregister-only, close kills the tmux window)
- `xscore data read/write/resize` over pane-id (NOT link-id)
- `xscore config show/path/set/unset` operates on `~/.config/xscore/config.json`; `path` prints all 3 dirs (config, links, state)
- Internal routing: `LinkAddress` Hash impl + `LinkRegistry::find_by_address` are internal helpers only — not a user-facing dedup key (ADR 0005)

---

## What's deferred (explicitly NOT in this slice)

- Real russh connect + pty channel (Task 8 skeleton; SSH-integration phase)
- Real serial port open (Task 8 skeleton; serial-integration phase)
- `link open` for `ssh` — needs real russh channel + tmux over the wire (lands in SSH-integration)
- `Sink::attach()` for "remote" mode in TmuxSink (current no-op; will wire when SshTransport gets real IO)
- Live streaming subscribe (`PaneBackend::snapshot` only for now)
- Tmux per-char send-keys perf optimization
- Cross-process manager state (CLI = in-memory; MCP server = daemon later). `state/links/<id>.json` is reserved but unused this slice.
- MCP server, IPC server, web UI — land in subsequent slices once this surface stabilizes
- RawSink full implementation (`?tmux=off` today is a no-op; tracked in open question)
- `link open <link-id>` — there is no `link open <link-id>` verb; tmux sessions are addressed by URL/name (`link open <url>`), not by link-id. `pane open <pane-id>` is a stub (`todo!()`) — full re-attach implementation lands in attach-persistence PR.

---

## Open questions (will be revisited, do not block the slice)

1. **Link uniqueness across CLI invocations.** Today `LinkRegistry` is in-memory; restarting `xscore` forgets all links + their panes. Real dedup across runs requires a persistence layer (likely a daemon). For this slice, `link open` covers tmux sessions within a single CLI invocation via the external tmux server (the server keeps running across CLI invocations), but discovery of tmux sessions created in an earlier invocation requires the persisted state file.
3. **`?tmux=off` semantics.** RawSink is a no-op stub for capture/write. Future PR will wire it to a passthrough mode.
4. **TmuxSink "auto" mode.** Resolves to `local` for shell/serial, `remote` for ssh. Future: explicit `tmux_socket` query param to override.
5. **Pane persistence.** Closing the CLI forgets the in-memory pane registry; `tmux -L xscore` windows keep running but are invisible to `pane list`. `pane open <pane-id>` (when link is also attached) would address this — see "deferred" list.

---

## Execution Handoff

This plan is split across files in `doc/dev/roadmap/`:

- `overview.md` — architecture, modules, core traits, CLI surface
- `task-NN-*.md` — one file per task (sequential edits only need to read that one file)
- `acceptance.md` — this file

Adjacent directories (also tracked in git):

- `../adr/0001-link-pane-model.md`, `0002-transport-sink-split.md`, `0003-external-tmux-server.md`, `0005-add-open-close-del-verbs.md` — design rationale (0004-link-existing-dedup.md is archived under `archive-2026-10-16-adr-0004-reversal/`)
- `../architecture/diagrams/README.md` — Link×Pane topology, lifecycle, module-deps rendered as inline Mermaid (parse-validated against mermaid@9)

Next step: execute via `subagent-driven-development` skill — fresh subagent per task, two-stage review (spec compliance then code quality). Each task ends with `cargo test -p xscore` + `cargo build -p xscore` before commit.

Want me to proceed with execution, or revise the slice first?