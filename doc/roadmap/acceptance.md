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
- `xscore window add --link <link-id> [--name=NAME]` creates a tmux window inside the link's session
- `xscore window list [--link <link-id>]` enumerates windows in a link
- `xscore window show --id <window-id>` prints WindowInfo JSON
- `xscore window del --id <window-id>` kills the tmux window (panes inside die)
- `xscore link add` auto-creates one default window + one default pane
- `xscore pane add <link-id> [--name=...]` opens a new pane inside a live link
- `xscore pane add --window <window-id> [--name=...]` opens a new pane inside a window (ADR 0006)
- `xscore pane list/show/del` operates on in-memory panes (`del` is full teardown — kills tmux split + unregisters)
- `xscore data read/write/resize` over pane-id (NOT link-id, NOT window-id)
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
- `link open <link-id>` — there is no `link open <link-id>` verb; tmux sessions are addressed by URL/name (`link open <url>`), not by link-id. There is also no `pane open` verb; cross-invocation re-attach happens via `link open` at the link level.

---

## Open questions (will be revisited, do not block the slice)

1. **Link uniqueness across CLI invocations.** Today `LinkRegistry` is in-memory; restarting `xscore` forgets all links + their panes. Real dedup across runs requires a persistence layer (likely a daemon). For this slice, `link open` covers tmux sessions within a single CLI invocation via the external tmux server (the server keeps running across CLI invocations), but discovery of tmux sessions created in an earlier invocation requires the persisted state file.
3. **`?tmux=off` semantics.** RawSink is a no-op stub for capture/write. Future PR will wire it to a passthrough mode.
4. **TmuxSink "auto" mode.** Resolves to `local` for shell/serial, `remote` for ssh. Future: explicit `tmux_socket` query param to override.
5. **Pane persistence.** Closing the CLI forgets the in-memory pane registry; `tmux -L xscore` windows keep running but are invisible to `pane list`. `xscore link open <url>` re-attaches the link and re-discovers panes from `sink.list_panes()` — see "deferred" list.

---

## Execution Handoff

This plan is split across files in `doc/dev/roadmap/`:

- `overview.md` — architecture, modules, core traits, CLI surface
- `task-NN-*.md` — one file per task (sequential edits only need to read that one file)
- `acceptance.md` — this file

Adjacent directories (also tracked in git):

- `../adr/0001-link-pane-model.md`, `0002-transport-sink-split.md`, `0003-external-tmux-server.md`, `0005-add-open-close-del-verbs.md`, `0006-window-concept.md` — design rationale (0004-link-existing-dedup.md is archived under `archive-2026-10-16-adr-0004-reversal/`)
- `../architecture/diagrams/README.md` — Link × Window × Pane topology, lifecycle, module-deps rendered as inline Mermaid (parse-validated against mermaid@9)

Next step: execute via `subagent-driven-development` skill — fresh subagent per task, two-stage review (spec compliance then code quality). Each task ends with `cargo test -p xscore` + `cargo build -p xscore` before commit.

Want me to proceed with execution, or revise the slice first?