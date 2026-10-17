# ADR 0003: Use an external tmux server, never embed tmux

- **Status:** Accepted
- **Date:** 2026-10-16
- **Deciders:** dev
- **Related:** [overview.md](../roadmap/overview.md), [ADR 0001](./0001-link-pane-model.md)

## Context

The sink layer needs PTY/line-buffering/ANSI handling. Options:

1. Implement a PTY in Rust (`portable-pty` crate, then our own line buffer / resize protocol).
2. Wrap an external tmux server via `tmux -L xscore ...` CLI commands.
3. Wrap an embedded tmux (libtmux — Python only; tmux-rs doesn't exist as a usable crate).

Each backend's `write`/`read` would otherwise have to know about resize, scrollback, line buffering. That's a lot of code to write and to keep correct.

## Decision

**External tmux server, addressed by `-L xscore` socket name.**

`TmuxSink::create` runs `tmux -L xscore new-session -d -s <link-name>` and stores the session name. `write` is `tmux send-keys`. `snapshot` is `tmux capture-pane -p -J`. `resize` is `tmux resize-pane`. `close` is `tmux kill-session`.

Per-link tmux session name = `xscore-link-<link_uuid>`. Per-pane (today's model) = `tmux new-window` inside that session. The tmux server is **one global `-L xscore` socket**, shared across every link — `xscore link add` is `tmux -L xscore new-session -d -s xscore-link-<uuid>`, not a per-link server.

```
xscore link add shell:///bin/cat
  → tmux -L xscore new-session -d -s xscore-01928e0a ...
  → pane #0 = default window in that server
  → xscore data write ... --pane ...  → tmux -L xscore send-keys -t xscore-01928e0a:0
```

## Consequences

**Positive:**
- PTY, line buffering, ANSI parsing, scrollback, resize — all done. We just call tmux.
- Debugging: any user can `tmux -L xscore attach` to see the live state. `tmux -L xscore ls` lists all links.
- Multi-pane on one link = multi-window in one tmux session (the session lives in the shared `-L xscore` server). Falls out naturally.
- "Tmux running inside SSH" becomes a Sink config: spawn tmux over the SSH channel via `Sink::attach()`. Deferred to SSH-integration phase but the seam exists.

**Negative:**
- External dependency: tmux binary must be installed. Document in install instructions.
- Per-char `send-keys` is wasteful (one tmux spawn per char). Acceptable for now; optimization is `send-keys -l <string>` (literal mode) which lands later.
- `TmuxSink::attach()` for "remote mode" (ssh channel) is a forward-compat no-op today; real wiring is non-trivial (russh channel → tmux needs tmux over the wire). Deferred to SSH-integration.

**Risks:**
- If tmux server is killed externally, link state becomes inconsistent. Mitigation: `TmuxSink::del()` is best-effort kill-session; link is also removed from `LinkRegistry` when `del_link` (formerly `close_link`) runs. The tmux server is reused across CLI invocations because we don't kill it on `xscore` exit — this is a feature (sessions keep running, re-attachable via `link open`) but means stale servers can accumulate. Mitigation: documentation + a future `xscore link list --all` flag.

## Alternatives considered

- **`portable-pty` + custom line parser.** Decided. 80%+ of the work would be reimplementing tmux's protocol. Not worth it.
- **`tmux-rs` crate.** Doesn't exist as a usable crate. (A few abandoned attempts; not maintained.)
- **`libtmux` (Python).** Rejected: would require an embedded Python interpreter.
- **`Mosh` server.** Out of scope; mosh solves a different problem.