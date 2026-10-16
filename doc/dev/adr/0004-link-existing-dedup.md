# ADR 0004: `--link=existing` semantics (opt-in address-based dedup)

- **Status:** Accepted
- **Date:** 2026-10-16
- **Deciders:** dev
- **Related:** [ADR 0001](./0001-link-pane-model.md), [overview.md](../roadmap/overview.md)

## Context

Two `xscore link add ssh://u@h:22` calls in sequence could mean:

- (a) Two independent connections (current default in the original flat-session model).
- (b) One connection with two sessions.
- (c) One connection with one session that the second call attaches to.

Each has different cost. (a) opens two SSH channels. (b)/(c) share one.

The user might want any of the three depending on context:

- I have two unrelated dev tasks on the same host → I want (a).
- I want to compare two views of the same remote state → I want (b)/(c).
- I want to multiplex shell commands without re-handshaking → I want (c).

Making any one of these the silent default would surprise the others.

## Decision

**`xscore link add <url>` without flags = behavior (a).** Always creates a new link.

**`xscore link add <url> --link=existing` = behavior (b)/(c) with attach-style reuse.** Searches the `LinkRegistry` for an existing link matching the URL's `LinkAddress`. If found, attaches a new pane to it. If not found:

- For `shell` / `serial` transports: returns `Error::LinkExistingUnsupported("<transport>")` — there is no meaningful "same link" semantics for these (a shell process is one process; a serial port is one port).
- For `ssh` transport: returns `Error::LinkNotFound(...)` today. Real attach (open an SSH channel and run `tmux -L xscore` over it) lands in the SSH-integration PR.

`LinkAddress` is hashable and derived deterministically from the URL:

```
shell://...   → (command, args)
serial://...  → (path)
ssh://...     → (host, port, user, identity, proxy)
```

Two URLs with the same `LinkAddress` tuple map to the same link.

The related verbs `xscore link open <link-id>` (re-attach by id) and `xscore pane open <pane-id>` (re-attach by pane id) are **stubs** in this slice — they call `todo!()` and return `Error::AttachNotImplemented`. See `task-10-cli-connect-link-pane.md`. Future PR wires them via persistent link-state files under `~/.config/xscore/state/links/`.

## Consequences

**Positive:**
- Predictable: the user must opt in to reuse. Default is independent, which is what most people want when they open two terminal tabs.
- The CLI surface has one unambiguous behavior per flag combination.
- `LinkAddress` becomes the single source of truth for "are these URLs the same link?". Easy to test, easy to reason about.

**Negative:**
- Users who wanted automatic dedup must add `--link=existing` to their muscle memory. Document loudly in `--help`.
- Two URLs that differ only in a query arg that doesn't affect `LinkAddress` (e.g. `?tmux=on` vs `?tmux=off`) still count as the same link. Acceptable: tmux mode is a Sink config, not an address component.

**Risks:**
- A user with two SSH configs to the same host (different identities) does see them as different links. Correct — different identity = different `LinkAddress`.
- `LinkAddress` for ssh currently lacks the *remote tmux server name* (because we don't yet have one). When the SSH-integration phase lands and runs tmux inside the SSH channel, the remote tmux server name becomes the link key on the remote — this is a future ADR.
- Per-process registry only — restarting `xscore` forgets all links. Documented as open question #1 in `acceptance.md`.

## Alternatives considered

- **Always dedup by `LinkAddress`.** Rejected: silently changes behavior for users running two tabs to the same host.
- **Tag-based dedup (user picks a label).** Rejected: extra UX surface for marginal benefit. `LinkAddress` is automatic.
- **Dedup only if `?link=<name>` query is set.** Rejected: makes the common dedup case verbose. Reuse should be the *opt-in* exception, not the default.