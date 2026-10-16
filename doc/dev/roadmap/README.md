# xscore CLI First Implementation Plan

This directory holds the implementation plan for the CLI-first slice of xscore — a Rust
workspace that manages SSH / shell / serial sessions behind a single `Link × Pane`
abstraction.

The plan is split across files. **Each task lives in its own file so edits stay scoped**
— when revising a task, read only that file.

## Layout

| File | What it covers |
| --- | --- |
| `overview.md` | Architecture (Link × Pane), module tree, hard module rules, core trait design preview |
| `task-01-workspace-bootstrap.md` | Rust workspace + crate skeleton |
| `task-02-core-ids-error-address.md` | `LinkId`, `PaneId`, `LinkAddress`, `core::Error` |
| `task-03-url-parsing.md` | URL → `(ConnectSpec, LinkAddress)` parser |
| `task-04-transport-trait.md` | `Transport` trait + echo stub |
| `task-05-sink-trait-tmux-raw.md` | `Sink` trait + per-pane ops, `TmuxSink`, `RawSink`, factory |
| `task-06-link-pane-registries-manager.md` | `Link`, `Pane`, three registries, `SessionManager.connect` |
| `task-07-shell-transport.md` | `ShellTransport` real impl |
| `task-08-transport-stubs.md` | `SerialTransport` + `SshTransport` stubs |
| `task-09-config-loader.md` | XDG paths + `ConnectSpec` JSON save/load (under `links/`) |
| `task-10-cli-connect-link-pane.md` | `xscore link {add,open,close,remove,show,list,config ...}` + `xscore pane {...}` CLI |
| `task-11-cli-data.md` | `xscore data read/write/resize` (per-pane) |
| `task-12-cli-config.md` | `xscore config show/path/set/unset` |
| `task-13-tests-readme.md` | CLI smoke tests + README rewrite |
| `acceptance.md` | Acceptance criteria, deferred items, open questions, execution handoff |

## How to read

1. Start at `overview.md` for the architecture / module tree / trait previews.
2. Walk `task-01-…md` through `task-13-…md` in order. Each file is self-contained —
   no need to re-read the whole plan to revise one task.
3. `acceptance.md` is the final state you should be able to satisfy.

## How to edit

When changing a task, **patch its file in place**. No file should grow past a few hundred
lines; if a task starts ballooning, split it into a `task-XX-a-…md` and `task-XX-b-…md`
pair and update this index.

When adding a new task, append `task-NN-<name>.md` with the next number and add a row to
the table above.

## Quick reference

- **Project root:** `/home/loner/prj/xscore/`
- **Crate:** `xscore` (single binary)
- **CLI entry:** `xscore link add <url> [-c CONFIG] [-a ALIAS]`
- **Two-tier CLI:** `xscore {link, pane, data, config} ...`
- **Persistent templates:** `~/.config/xscore/links/<alias>.json` (`link config add`)
- **Config dir:** `~/.config/xscore/`
- **tmux socket:** `xscore` (`-L xscore`)
- **Env prefix:** `XSCORE_*`