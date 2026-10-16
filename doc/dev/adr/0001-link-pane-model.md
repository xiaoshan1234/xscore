# ADR 0001: Model sessions as Link × Pane (not flat Session)

- **Status:** Accepted
- **Date:** 2026-10-16
- **Deciders:** dev
- **Related:** [overview.md](../roadmap/overview.md), [diagrams/link-pane-topology.svg](../architecture/diagrams/link-pane-topology.svg)

## Context

The original design (`xsterm` prototype) treated a session as a single unit: one TCP connection / one SSH channel / one local process / one serial port. A user opening "the same session twice" had no clean way to share the underlying transport — every `connect` was a brand new unit.

Three product realities drove the change:

1. **Tmux-in-SSH is the norm.** When a developer connects to a remote box over SSH, they want `tmux` running *inside* that SSH channel. The "session" they're working with is `ssh-channel` + tmux-server + N windows. The previous single-session model treated each tmux window as a separate SSH connection, which is wrong.
2. **Same link, multiple panes.** A single SSH connection can hold multiple useful workspaces (one window per task). The model must let one link host N panes without re-opening the SSH channel.
3. **Address-based dedup.** Two `xscore link add ssh://u@h:22` calls — without explicit flags — should not both open SSH channels to the same host. The user opts in with `--link=existing` to attach.

## Decision

Introduce a **two-level model**:

- **Link** = one physical connection. Owns a `Transport` + `Sink` + a `Vec<PaneId>`. Identified by `LinkId` (uuid v7) and a stable `LinkAddress` hashable key derived from the transport tuple.
- **Pane** = one view into a link. Identified by `PaneId` (uuid v7). Has independent lifecycle, snapshot, write state. Addressed by `sink_handle` (e.g. tmux `%42`).

Every CLI command operates on either a `link-id` or a `pane-id` — never a generic "session id".

In addition, each alive link may carry a **runtime alias** (`xscore link add ... -a <alias>`) for human-readable reference. Aliases live in `LinkRegistry.by_alias` (in-memory only, lost on process exit) and are not the dedup key — `LinkAddress` is. Persistent templates use a separate `link config add <alias> <url>` flow under `~/.config/xscore/links/`.

```
SessionManager
├─ LinkRegistry (link_id -> Arc<Link>, address -> LinkId)
├─ PaneRegistry (pane_id -> PaneInfo)
└─ BackendRegistry (pane_id -> Arc<dyn PaneBackend>)
```

## Consequences

**Positive:**
- `--link=existing` becomes a clean semantic: "find an existing link by `LinkAddress`; attach a new pane to it; if none, error."
- Closing a pane does not affect the link. Killing a link kills every pane on it.
- `TmuxSink` owns one tmux server (the link); `new_pane` is `tmux new-window`. The "multi-window in one tmux server" semantics fall out for free.
- Future "live attach" (a second CLI invocation watches an existing link) is just a question of where the registry lives (in-memory today; daemon later). The model is correct.

**Negative:**
- Two IDs instead of one. CLI surface grows: `link list`/`pane list`. Documentation must explain the distinction clearly.
- `Sink` trait gains `new_pane`/`close_pane`/`list_pane`-per-handle methods. More boilerplate than a single-session trait, but each method is small.
- `xscore link add <same url>` (without `--link=existing`) is now the *default* behavior — users expecting dedup must opt in. This inverts the prior assumption. Documented explicitly in `--help` text and `acceptance.md`.

**Risks:**
- Users conflate link-id with pane-id and `data --id` style commands become ambiguous. Mitigated by `--pane` flag on `data` (not `--id`).
- The `LinkAddress` for ssh currently includes `(host, port, user, identity, proxy)`. A future ssh config change (e.g. default identity switches) would silently make the same URL map to a different address. Acceptable; the user can `--link=existing` explicitly.

## Alternatives considered

- **Flat SessionId only.** Rejected: forces N SSH channels per "session-group", makes tmux-in-ssh awkward.
- **Three-level (Host → Connection → Session).** Overkill — we don't run multiple connections per host in this slice.
- **Tag-based dedup.** User picks a label, dedup is label-keyed. Rejected: tag namespace not designed, adds UX surface for marginal benefit. `LinkAddress` (derived from URL) is the dedup key.