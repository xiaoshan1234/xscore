# ADR 0005: `link` verbs are `add` / `del` / `open` / `close` (no `--link=existing`)

- **Status:** Accepted
- **Date:** 2026-10-16
- **Deciders:** dev
- **Supersedes:** [ADR 0004](./../archive-2026-10-16-adr-0004-reversal/0004-link-existing-dedup.md) — opt-in address-based dedup via `--link=existing` is retired.
- **Related:** [ADR 0001](./0001-link-pane-model.md), [overview.md](../roadmap/overview.md), [task-10](../roadmap/task-10-cli-connect-link-pane.md)

## Context

ADR 0004 introduced `--link=existing` as an opt-in flag on `xscore link add` to express "attach a new pane to an existing link by `LinkAddress`". The user reviewed that design and rejected it: the flag conflates two distinct user intents ("create a new resource" and "re-attach an existing one") behind a single verb, and the rejection surface (`Error::LinkExistingUnsupported` for shell/serial; not-yet-implemented for ssh) was an admission that the abstraction didn't fit the transports anyway.

The reasoning behind the cut: tmux is the only transport where an "attach existing session by name" semantic is meaningful. Shell processes, serial ports, and one-shot SSH channels have no cross-invocation identity — they exist only while the CLI holds them. Forcing these transports through an "address-keyed dedup" layer added concepts that didn't apply, not just rules.

## Decision

Drop `--link=existing` entirely. Replace it with explicit verb semantics on `xscore link`:

```
xscore link add   <url?> [-c CONFIG]    # create a new external resource (always)
xscore link open  <url?> [-c CONFIG]    # attach an existing external resource (tmux only)
xscore link close --id <link-id>        # detach: unregister from in-memory only (tmux only)
xscore link del   --id <link-id>        # full teardown: kill the external resource
```

`-c CONFIG` (the persistent template lookup) remains a flag on both `add` and `open`, and is optional on both: at least one of `<url>` or `-c CONFIG` must be supplied.

### Per-verb semantics

| Verb    | External resource? | In-memory registry? | Allowed transports | What the resource does |
|---------|-------------------|---------------------|--------------------|------------------------|
| `add`   | creates new       | inserts new link    | all (shell/serial/ssh/tmux) | tmux: `tmux new-session -d -s <name> -t xscore`; others: native spawn/connect |
| `open`  | re-attach only    | inserts new link record for it | tmux only — non-tmux returns `Error::OpenOnlyForTmux(<kind>)` | tmux: attach to existing session by name in the shared `-L xscore` server |
| `close` | unchanged         | removes link record only | tmux only — non-tmux returns `Error::CloseOnlyForTmux(<kind>)` | tmux session keeps running inside the shared server |
| `del`   | kills it          | removes link record | all | full teardown of transport + sink + every pane + (for tmux) `tmux kill-session -t xscore-link-<link_uuid>` (the shared `-L xscore` server stays up) |

A `close` on a tmux link leaves the tmux session alive inside the shared `-L xscore` server; a subsequent `open` against the same session name succeeds. The session lives across CLI invocations.

### `LinkAddress` is now an internal-only concept

The hashable `LinkAddress` that ADR 0004 used for dedup becomes an internal layer: it's still useful inside the manager for routing (e.g. "where do I send a write?"), but it is no longer a user-facing key. `LinkRegistry.find_by_address` survives only as an internal helper. Users identify links by `link-id` (uuid v7) at the CLI; "create new" vs "attach existing" is decided by the verb, not by a flag.

### Persistent templates (`link config ...`) are unchanged

The `link config {add,remove,show,list} <name> <url>` sub-tree (locked in ADR 0004's neighborhood, the prior turn's cut) is untouched. `-c <name>` still reads a saved `ConnectSpec` template from `~/.config/xscore/links/<name>.json` and merges the CLI URL on top.

### Manager method mapping

| Old name (task-06 / task-10) | New name | What changed |
|------------------------------|----------|--------------|
| `add_link(spec, address, link_existing: bool)` | `add_link(spec, address)` | third param dropped; no `--link=existing` branch |
| `attach_link(id)` (stub) | `open_link(spec, address)` | renamed; signature changes from id-based to spec-based (resolve to tmux session) |
| `close_link(id)` (full teardown) | `del_link(id)` | renamed; semantically unchanged |
| `unregister_link(id)` (in-memory only) | `close_link(id)` | renamed; semantically unchanged |
| n/a | `open_link` `close_link` `del_link` | error variants `OpenOnlyForTmux`, `CloseOnlyForTmux`, `KillFailed` exposed via `core::Error` |

### Error variants

- `Error::LinkExistingUnsupported` — **deleted**.
- `Error::OpenOnlyForTmux(&'static str)` — **new**. `link open` against shell/serial/ssh returns this.
- `Error::CloseOnlyForTmux(&'static str)` — **new**. `link close` against shell/serial/ssh returns this.
- `Error::AttachNotImplemented` — deleted; no `pane open` verb exists (cross-invocation re-attach happens via `xscore link open`).

## Consequences

**Positive:**
- Each verb does what its name says. No flags that quietly change behavior.
- The transport-specific error surface is honest: `link open` against a shell is rejected immediately, rather than after a `LinkAddress` lookup discovers no matching link.
- The address-keyed dedup codepath (`find_by_address` + `Error::LinkExistingUnsupported`) is removed entirely, simplifying both `task-06` and `task-10`.
- Tmux becomes the one place where the "attach by name" semantic lives, which matches where it actually applies.

**Negative:**
- `link open` is a tmux-only verb — for shell/serial/ssh links the user can't re-attach by id across CLI invocations. This was already true in practice (the original `attach_link` was a stub); making it explicit in the CLI just surfaces the limitation.
- The 4-verb `link` surface has tighter coupling to tmux semantics than the 4-verb surface had under the prior design. Users running only shell/serial will see two verbs (`open`, `close`) that always fail with a transport error.

**Risks:**
- Restoring the `LinkAddress` as internal-only means future code that wants to "list all links to host X" can still do so, but isn't a public API. Acceptable; if a future CLI command needs it, that's a new ADR.
- This is the third `link` CLI design in the project. There's no guarantee it's the last — but each cut has shrunk the surface (alias cut → `--link=existing` cut), so the trajectory is toward fewer concepts, not more.

## Alternatives considered

- **Keep `--link=existing` but make it work for tmux only.** Rejected: still couples create-time to attach-time intent behind a flag.
- **One verb `link` plus a sub-command inside tmux plumbing.** Rejected: pushes the same shape into the transport layer, with worse discoverability.
- **Use verbs borrowed from tmux itself (`new-session`, `attach-session`, `detach-client`, `kill-server`).** Rejected: makes shell/serial/ssh feel like second-class citizens in the API. The 4-verb model is transport-agnostic at the surface and tmux-specific in implementation — the abstraction is clean.
