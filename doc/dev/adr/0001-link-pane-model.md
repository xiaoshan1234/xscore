# ADR 0001: Model sessions as Link × Window × Pane (not flat Session)

- **Status:** Accepted
- **Date:** 2026-10-16
- **Deciders:** dev
- **Related:** [overview.md](../roadmap/overview.md), [diagrams/link-pane-topology.svg](../architecture/diagrams/link-pane-topology.svg), [ADR 0006](./0006-window-concept.md) (extends the model with a Window tier between Link and Pane)

## Context

The original design (`xsterm` prototype) treated a session as a single unit: one TCP connection / one SSH channel / one local process / one serial port. A user opening "the same session twice" had no clean way to share the underlying transport — every `connect` was a brand new unit.

Three product realities drove the change:

1. **Tmux-in-SSH is the norm.** When a developer connects to a remote box over SSH, they want `tmux` running *inside* that SSH channel. The "session" they're working with is `ssh-channel` + tmux-server + N windows. The previous single-session model treated each tmux window as a separate SSH connection, which is wrong.
2. **Same link, multiple panes.** A single SSH connection can hold multiple useful workspaces (one window per task). The model must let one link host N panes without re-opening the SSH channel.

**Cross-process attach (separate from this ADR).** The model says nothing about how `xscore` discovers a link across CLI invocations — today the registry is in-memory only. Re-attach across invocations is owned by ADR 0005 (`add` / `open` / `close` / `del`), which routed the attach semantic through tmux's per-session identity instead of `LinkAddress`. The original "address-keyed dedup" concept that this ADR's first draft mentioned is no longer part of the design.

## Decision

Introduce a **three-level model** (ADR 0006 extends this further with a Window tier):

- **Link** = one physical connection. Owns a `Transport` + `Sink` + a `Vec<PaneId>`. Identified by `LinkId` (uuid v7) and a stable `LinkAddress` hashable key derived from the transport tuple.
- **Pane** = one view into a link. Identified by `PaneId` (uuid v7). Has independent lifecycle, snapshot, write state. Addressed by `sink_handle` (e.g. tmux `%42`).

(With Window added per ADR 0006: Link owns a `Vec<WindowId>` instead; each Window owns a `Vec<PaneId>`; Pane's parent is `WindowId` not `LinkId`. The 2-tier bullet above is the "Link × Pane" view that 99% of the docs work in; the 3-tier view is the full Link × Window × Pane model. Documented first as 2-tier for legibility, then extended.)

Every CLI command operates on either a `link-id` or a `pane-id` — never a generic "session id".

```
SessionManager
├─ LinkRegistry (link_id -> Arc<Link>, address -> LinkId)
├─ PaneRegistry (pane_id -> PaneInfo)
└─ BackendRegistry (pane_id -> Arc<dyn PaneBackend>)
```

## Consequences

**Positive:**
- `link open` becomes a clean semantic: "attach to an existing tmux session by name; if none, error." (For shell/serial transports the verb returns `Error::OpenOnlyForTmux` — the surface invariant in ADR 0005.)
- Closing a pane does not affect the link. Killing a link kills every pane on it. Detaching (`link close`) keeps the tmux session alive but unregisters the link in-memory; killing (`link del`) does both. See ADR 0005.
- `TmuxSink` owns one tmux session (the link) inside the shared `-L xscore` server; `new_pane` is `tmux new-window`. The "multi-window in one tmux session" semantics fall out for free.
- Future "live attach" (a second CLI invocation watches an existing link) is just a question of where the registry lives (in-memory today; daemon later). The model is correct.

**Negative:**
- Two IDs instead of one. CLI surface grows: `link list`/`pane list`. Documentation must explain the distinction clearly.
- `Sink` trait gains `new_pane`/`close_pane`/`list_pane`-per-handle methods. More boilerplate than a single-session trait, but each method is small.
- `xscore link add <same url>` always creates a **new** link — there is no `--link=existing` to opt into reuse. To attach to an existing tmux session the user runs `link open <url>` instead (ADR 0005). This is explicit at the verb level; the prior opt-in was implicit.

**Risks:**
- Users conflate link-id with pane-id and `data --id` style commands become ambiguous. Mitigated by `--pane` flag on `data` (not `--id`).
- The `LinkAddress` for ssh currently includes `(host, port, user, identity, proxy)`. It's an internal concept now (ADR 0005 retired `--link=existing`); only used inside `LinkRegistry` for routing. No further change implied.

## Alternatives considered

- **Flat SessionId only.** Rejected: forces N SSH channels per "session-group", makes tmux-in-ssh awkward.
- **Three-level (Host → Connection → Session).** Overkill — we don't run multiple connections per host in this slice.
- **Tag-based dedup.** User picks a label, dedup is label-keyed. Rejected: tag namespace not designed, adds UX surface for marginal benefit. `LinkAddress` (derived from URL) is the dedup key.