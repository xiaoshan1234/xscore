# ADR 0006: Window — a 3rd-tier logical concept between Link and Pane

- **Status:** Accepted (Revised 2026-10-19 — see [ADR 0007](./0007-raw-default-tmux-opt-in.md); 3-tier model applies to tmux-backed links only)
- **Date:** 2026-10-16 (revised 2026-10-19)
- **Deciders:** dev
- **Related:** [ADR 0001](./0001-link-pane-model.md), [ADR 0003](./0003-external-tmux-server.md), [ADR 0005](./0005-add-open-close-del-verbs.md), [ADR 0007](./0007-raw-default-tmux-opt-in.md) (the 2026-10-19 design pivot that narrowed this ADR's scope)

## Context

xscore exposes two addressable tiers today; the tmux server is **shared infrastructure** (one global `-L xscore` socket per ADR 0003), not a tier:

```
Link  (transport + sink + per-link tmux session inside the shared -L xscore)
 └─ Pane (== tmux window inside that session)
```

Each `xscore link add` allocates one tmux session via `tmux -L xscore new-session -d -s xscore-link-<link_uuid>`. Each `xscore pane add` is `tmux new-window` directly inside that session. We collapse tmux's "tmux window" tier into our "Pane" tier — what we call a pane, tmux would call a window.

**tmux's full hierarchy** is four tiers: **server** → **session** → **window** → **pane**. Our two addressable tiers (link / pane) skip the "tmux window" tier. Three visible costs:

1. **Mental-model mismatch with tmux users.** A tmux-native user thinks in sessions (`tmux new-session -s dev`, `-s ops`, `-s play`) and windows (tabs inside a session). Today, "I want three work contexts in one ssh session" requires three xscore links — three tmux sessions, even though tmux keeps them siblings inside one shared `-L xscore` server. The model forces them apart, the wire keeps them together.

2. **Cross-invocation attach granularity is too coarse.** With `xscore link open <url>` re-attaching the whole link, all the link's panes come back — there's no way to re-attach just one logical "tmux window." `tmux switch-client -t <window>` does this in tmux; we don't.

3. **tmux UI integration gets awkward.** A tmux-native tool scanning the shared `xscore` server sees all sessions (one per link), all windows (today: mapped to our panes), and can't distinguish "a session I want, with its own windows" from "a session that just has many windows."

(Note: an earlier draft of this ADR wrote Link as "tmux server" with `tmux -L xscore-<link_uuid>` — that was wrong. The tmux server is one global `-L xscore` per ADR 0003; what we call a Link is the *session inside that server*, and what we call a Pane hides what tmux calls a window. The user's correction flipped the mapping. ADR 0003 line 24 carries the same wording bug.)

This ADR proposes inserting a **Window** tier between Link and Pane so the model matches tmux's natural shape **for the addressable tiers** (server is shared infrastructure; not a tier).

## Proposed decision

Add a Window as the third tier:

```
Link    (transport + sink + per-link tmux session inside shared -L xscore)
 └─ Window  (== tmux window, named within its link)
     └─ Pane (== tmux pane, split inside the window)
```

**Mapping cheat sheet** (this is what every doc must say internally — fix the rest to match):

| xscore tier | tmux tier | Created by | Killed by | Storage |
|---|---|---|---|---|
| (none — shared) | tmux server | `tmux -L xscore` (already up; ADR 0003) | n/a | Outside xscore; one global socket |
| `Link` | tmux session | `tmux -L xscore new-session -d -s xscore-link-<link_uuid>` | `tmux -L xscore kill-session -t xscore-link-<link_uuid>` | `LinkRegistry` by `LinkId` |
| `Window` | tmux window | `tmux -L xscore new-window -t xscore-link-<link_uuid> -n <name>` | `tmux -L xscore kill-window -t xscore-link-<link_uuid>:<window_idx>` | `WindowRegistry` by `WindowId` |
| `Pane` | tmux pane | `tmux -L xscore split-window -t xscore-link-<link_uuid>:<window_idx>` | `tmux -L xscore kill-pane -t xscore-link-<link_uuid>:<window>.<pane>` | `PaneRegistry` by `PaneId` |

(Indexed session/window/pane selectors like `<link_uuid>:<idx>` follow tmux's own address syntax; treat this as informal until implementation time — tmux has its own ways to reference these.)

### Identity and structure

- **`WindowId`** — uuid v7 (consistent with `LinkId` / `PaneId`).
- **`WindowInfo`** — `{ id, link_id, name, created_at, tmux_window_index }`. The `tmux_window_index` is what we use to address the window inside the link's session (tmux numbers windows starting at 0 in creation order; `:list-windows -t <session>` reorders them as the user renames and moves).
- **`WindowRegistry`** — sibling of `LinkRegistry` and `PaneRegistry`. Hash key is `WindowId`. Optional secondary index `link_id → Vec<WindowId>` for "list windows in a link."
- **`PaneInfo` change** — current `link_id` field becomes `window_id`. Pane → Window → Link is a single dereference; back-references by `link_id` mean going through `WindowRegistry`.

### CLI surface

New verb group, mirroring the pane tree's shape:

```
xscore window add   --link <link-id> [--name=NAME]   # new tmux window inside the link's session
xscore window list  [--link <link-id>]              # enumerate windows in a link
xscore window show  --id <window-id>
xscore window del   --id <window-id>                 # tmux kill-window (panes inside die)
```

Pane verbs change `--link` → `--window`:

```
xscore pane   add   --window <window-id> [--name=NAME]   # split-window inside that window
xscore pane   list  [--window <window-id>]
xscore pane   show  --id <pane-id>
xscore pane   del   --id <pane-id>
```

The link tree's 4 lifecycle verbs (`add / del / open / close`) stay — `link open` is still the cross-invocation attach verb (the user's link = a tmux session on the shared server, so `link open <url>` re-attaches that session).

For the common case of "I want one window per link and one pane inside it," we may want a `link add` shortcut that auto-creates one default window so the user doesn't need to do `link add` + `window add` + `pane add`. Out of scope here; tracked under "Open questions."

### Manager method mapping (after acceptance)

| Old (current 2-tier) | New (3-tier) |
|---|---|
| `add_pane(link_id, name)` | `add_window(link_id, name)` + `add_pane(window_id, name)` |
| `del_pane(id)` | `del_pane(id)` (unchanged in shape; target is still `PaneId`) |
| `del_link(id)` | `del_link(id)` — now also kills the per-link tmux session (which kills every window + pane in it). The shared `-L xscore` server stays up. |
| n/a | `del_window(id)` — tmux kill-window inside the parent session; panes inside die with it. |

### tmux wire (corrected — server is shared, not per-link)

- **Link creation**: `tmux -L xscore new-session -d -s xscore-link-<link_uuid>` — creates one tmux session in the shared server, named so it doesn't collide with other xscore links.
- **Link deletion**: `tmux -L xscore kill-session -t xscore-link-<link_uuid>` — kills session + every window + every pane inside; server stays up.
- **Window creation**: `tmux -L xscore new-window -t xscore-link-<link_uuid> -n <window_name>` (or default name).
- **Window deletion**: `tmux -L xscore kill-window -t xscore-link-<link_uuid>:<window_idx>`.
- **Pane creation**: `tmux -L xscore split-window -t xscore-link-<link_uuid>:<window_idx>`.
- **Pane deletion**: `tmux -L xscore kill-pane -t xscore-link-<link_uuid>:<window_idx>.<pane_idx>`.

### Diagram changes

The two-tier diagram grows a middle layer. Both the topology diagram and the lifecycle state machine need a `Window` node. The state machine gets a new sub-machine for "active windows in a link" parallel to the existing "active panes in a window" structure.

## Acceptance

This ADR is **Proposed**, not Accepted. To accept it:

1. **Update ADR 0003** — line 24 currently reads "Per-link tmux server name = the `link-id` uuid"; that line conflates "server" with "session" and is the source of the bug this ADR corrects. Fix it to read: "Per-link tmux session name = `xscore-link-<link_uuid>`. Per-window = `tmux new-window` inside that session. The tmux server is shared (`-L xscore`)."
2. **Update ADR 0001** — `TmuxSink` description currently reads "owns one tmux server (the link)" — fix to "owns one tmux session (the link) inside the shared `xscore` tmux server."
3. **New task file** — `task-XX-window-registry.md` for `WindowId`, `Window`, `WindowInfo`, `WindowRegistry`.
4. **Updates to existing tasks**:
   - task-02 — add `WindowId` and `core::Window` types.
   - task-05 — add `new_window` / `kill_window` ops for windows inside `Sink::TmuxSink`; the session lifecycle (`new_session` / `kill_session`) is already needed today for link add/del.
   - task-06 — `WindowRegistry` plumbing in `SessionManager`; new `add_window` / `del_window` methods; `PaneInfo` field rename.
   - task-10 — `xscore window` subcommand; slice `pane --link` → `pane --window`.
   - task-13 — new smoke tests for window add/list/del; updated tests for pane add's `--window` flag.
5. **Updates to overview.md**, **acceptance.md**, **README.md** (top-level), **diagrams/README.md**, **roadmap/README.md** — sweep the "tmux server = link" wording everywhere it appears.
6. **Update ADR 0005** — its verb split stays correct; just adjust the wording in the "attach-existing" semantics table.

Estimated impact: ~12-14 files touched, comparable in size to ADR 0005's reversal.

## Consequences

**Positive:**
- The xscore addressable tiers (link → window → pane) match tmux's addressable tiers (session → window → pane); the server is shared infrastructure in both.
- A tmux-native user can reason about xscore using the same session/window/pane vocabulary.
- Cross-invocation re-attach at the window level becomes possible (a future `window open <name>`).
- Single tmux server (one global `-L xscore`) stays shared; only the *naming inside the server* gets richer.

**Negative:**
- +1 verb group (`window {add, del, show, list}`) = 4 more CLI verbs.
- `pane add --link` becomes `pane add --window` — breaking change for any user already scripting it (low impact: nothing's released yet).
- tmux wire gets one more op pair (`new-window` + `kill-window` per window) on top of the existing session ops.

**Risks:**
- A tmux session with one window vs. one window with one pane look similar in `pane list` output (a single `:window_id.pane_id`). The CLI commands must distinguish: link → window via `new-window`, window → pane via `split-window`. The two operations return different identifiers.
- `link close` (tmux-detach only — server lives, session lives, windows live) needs to detach every window inside without killing the session. With the current `close_link` implementation (kills server + sink — which the corrected model says should be: unregister in-memory, leave session alive), the windows must be tracked.

## Alternatives considered

- **Defer until tmux UI integration lands.** Keep the 2-tier model until something forces the 3-tier. **Rejected (currently)** because the user already flagged this as a missing concept; deferring means redoing the work later in a larger blast radius. Propose the 3-tier model now while implementation is still pre-code.
- **Make `Link` own multiple tmux sessions, with one shared server, but skip the Window tier.** I.e. have `add_window` only inside `link add --session <name>` — the user names tmux sessions directly without a separate Window concept. **Rejected** because the per-link-as-session picture already works (link name = session name); the missing tier is the **tmux-window-under-the-session**, which is what this ADR adds.
- **A 3-tier model where Window is opaque (no `WindowId`, only `name`).** **Rejected** because `name` is not unique across re-creation; uuids are needed for cross-invocation references.
- **Five tiers** (server / session / window-group / window / pane). **Rejected** as over-engineering for a CLI-first slice.

## Open questions

1. **`window open <name>` for cross-invocation re-attach at window level.** Same shape as `link open <url>` but window-scoped. Out of scope here; mention in ADR 0006 follow-up.
2. **Default window on `link add`.** Should `link add` auto-create one `<link_id>-default` window so users don't need `link add` + `window add` + `pane add` for the common case?
3. **tmux wired address syntax.** `<session>:<window_index>` is the natural form but tmux allows `<session>:<window_name>` too. Pick one at implementation time.
4. **Backwards compat.** Pre-release, so none — but if a pre-ADR-0006 alpha leak happened, those users would need a migration story.

## Decisions to make

Until the user accepts this ADR, none of the above is locked. The acceptance work above is the price of saying "yes."


## Revised 2026-10-19 (per ADR 0007): 3-tier model is for tmux-backed links only

This ADR's 3-tier model (Link → Window → Pane, mapping 1:1 to tmux's
session → window → pane) is no longer the universal model. The
2026-10-19 design pivot ([ADR 0007](./0007-raw-default-tmux-opt-in.md))
split the universe into two cases:

- **Tmux-backed links (`xscore link add <url>?tmux=on`):** the 3-tier
  model applies. Link = tmux session in the shared `-L xscore` server,
  Window = tmux window inside the session, Pane = tmux split-window
  inside the window. The auto-create window + pane on `link add` still
  happens. The CLI surface (window/pane subcommands, `data` subcommands)
  works as documented above.
- **Raw links (`xscore link add <url>`, no `?tmux=`):** the model is
  1-tier — Link = Transport. There is no Window, no Pane, no
  `xscore data` ops. The `xscore window` / `xscore pane` subcommands
  return `Error::NotTmuxLink(<link_id>)` for raw links.

The 3-tier model is the more powerful one (session persistence,
multi-window, multi-pane, attach from outside xscore via
`tmux -L xscore attach`). It's available to anyone who opts in via
`?tmux=on`. The 1-tier model is the simpler one (no tmux server in
the loop, no install requirement, no race bugs). It's the new default.

The mapping cheat sheet above (the tmux session / window / pane wire
table) is unchanged — it just applies to a narrower scope now
(tmux-backed links). The acceptance work listed below was completed
in commits `40f7a27` and `496d49f`; the `?tmux=...` opt-in semantics
were added in `efccc6b` along with the test + README + CHANGELOG
updates.

Read [ADR 0007](./0007-raw-default-tmux-opt-in.md) for the full
decision context and the migration notes from v0.1.0.
