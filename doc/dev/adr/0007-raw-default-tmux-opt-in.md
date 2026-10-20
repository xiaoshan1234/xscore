# ADR 0007: 3-entity decoupled model — Link × Pane × Window

> **Superseded in part by [ADR 0008](./0008-tmux-adoption.md) (2026-10-20)** — creation semantics changed from "no auto window/pane" to "auto-create + tmux adoption".

- **Status:** Accepted (rewrite — this ADR supersedes the previous
  ADR 0007 from 2026-10-19 which only narrowed raw vs tmux)
- **Date:** 2026-10-20 (first version: 2026-10-19)
- **Deciders:** dev
- **Related:** [ADR 0001](./0001-link-pane-model.md) (Link × Window × Pane
  split is now even cleaner — windows have NO link association; panes
  are orphan-creatable); [ADR 0002](./0002-transport-sink-split.md)
  (Sink is still optional — `RawSink` for raw links, `TmuxSink` for
  tmux-backed links); [ADR 0003](./0003-external-tmux-server.md)
  (still external, still on shared `-L xscore` server when used);
  [ADR 0006](./0006-window-concept.md) (Window is now a pure UI
  layout container — no `link_id`, no `tmux_window_index`)

## Context

The 2026-10-19 user clarification (after the v0.2.0 release) reframed
the entire model:

> "搞清楚一点，window 是纯ui概念，pane核心是buffer，带点ui信息，
> link是纯通信连接概念，建立link不要自动生成window或者pane，
> pane可以自由的切换link"

> "shell/ssh/serial 类型的link，只能属于一个pane，tmux 类型的link
> 可以被多个 pane 使用"

Translated:

- **Window** = pure UI concept (layout container)
- **Pane** = core is a buffer + a little UI info (geometry: cols/rows/x/y
  + which window it's in + which link it's attached to)
- **Link** = pure communication connection concept (transport)
- `xscore link add` creates ONLY the link — no auto window, no auto pane
- A pane can freely switch between links (detach from one, attach to another)
- **Q1 (confirmed 2026-10-19):** shell/ssh/serial link may have multiple
  panes attached OVER TIME (history), but only ONE is active at any
  time. When a new pane is attached, the previously-active pane is
  auto-detached (its buffer is kept).
- **Q2 (confirmed 2026-10-19):** pane's UI info = cols/rows/x/y geometry

The previous design (v0.2.0) was a 3-tier `Link → Window → Pane`
hierarchy with link-owned windows. The user rejected that. The new
design treats all three as INDEPENDENT entities. Associations are
dynamic via Option IDs on `PaneInfo`:

```
Pane.info.link_id: Option<LinkId>     // currently attached link
Pane.info.window_id: Option<WindowId> // currently in a window
Link.active_pane: Option<PaneId>     // Q1: tracks raw-link active pane
```

A pane can be created with neither a link nor a window (orphan); a
link can be created with no panes; a window can be created empty. The
user wires them together via `pane attach`, `pane detach`,
`pane add-to-window`, etc.

## Decision

### Three independent entities

```rust
// Link — transport + connection state (no windows/panes owned)
pub struct Link {
    pub id: LinkId,
    pub info: LinkInfo,
    pub transport: Arc<dyn Transport>,    // Arc so DirectBridge can clone
    pub sink: Box<dyn Sink>,              // Raw or Tmux
    pub tmux_sink: Option<Arc<TmuxSink>>, // concrete for bridge Weak
    pub active_pane: Mutex<Option<PaneId>>,  // Q1: direct-link tracking
}

// Pane — buffer + UI info (orphan-creatable)
pub struct PaneInfo {
    pub id: PaneId,
    pub name: String,
    pub created_at: DateTime<Utc>,
    pub cols: u16, pub rows: u16,         // Q2: geometry
    pub x: u16, pub y: u16,              // Q2: geometry
    pub link_id: Option<LinkId>,         // None = orphan
    pub window_id: Option<WindowId>,     // None = orphan
}

// Window — pure UI layout container (orphan-creatable, no link)
pub struct WindowInfo {
    pub id: WindowId,
    pub name: String,
    pub created_at: DateTime<Utc>,
    pub layout: WindowLayout,            // single | tabs | split_h | split_v | grid
    pub cols: u16, pub rows: u16,        // total window size
}
```

Each entity is created/owned independently. The CLI surfaces this:

```bash
xscore link add <url>           # only creates the link
xscore window add               # only creates a window
xscore pane add                 # only creates an orphan pane
xscore pane attach --pane <pid> --link <lid>   # wire pane → link
xscore pane add-to-window --pane <pid> --window <wid>  # wire pane → window
```

### Bridge model

Per-pane runtime is a `PaneBridge` trait with two concrete impls:

- `DirectBridge { transport: Arc<dyn Transport> }` — wraps the link's
  Arc. `read` is one-shot `transport.read(buf)`. Future: ring buffer
  sidecar (deferred to v0.4.0+).
- `TmuxBridge { sink: Weak<TmuxSink>, handle: String }` — wraps a weak
  ref to the link's concrete `TmuxSink`. `read` → `tmux capture-pane`;
  `write` → `tmux send-keys`; `resize` → `tmux resize-pane`; `close` →
  `tmux kill-pane`.

The pane stores `SessionManager.pane_bridges: Arc<Mutex<HashMap<PaneId, Arc<dyn PaneBridge>>>>`.
Bridges are stored SEPARATELY from `PaneInfo` so that mutating the info
(which is JSON-serializable) doesn't destroy the bridge (which is not).

### Q1 (1-active-pane for raw links)

When `attach_pane(pid, lid)` runs:

1. If `link.sink == Raw`:
   - Scan all panes for any OTHER pane with `info.link_id == Some(lid)`.
   - For each, close its bridge (silently) and clear its `link_id`.
   - Update `link.active_pane = Some(pid)`.
2. For tmux-backed links: NO auto-detach. Multiple panes can attach.

Pane info (persisted in state.json) is the source of truth — not the
in-memory `link.active_pane` cache. This makes Q1 work correctly across
process restarts.

### Q2 (pane geometry)

`PaneInfo` carries `cols`/`rows`/`x`/`y`. These are the pane's UI info
(Q2). The window's `cols`/`rows` are the total window size — independent
from pane geometry. Layout (single/tabs/split_h/split_v/grid) is a
hint stored on `WindowInfo`.

### State schema (BREAKING from v0.2.0)

```json
{
  "links":   [LinkInfo, ...],
  "windows": [{...WindowInfo..., "panes": ["<pid>", ...]}, ...],
  "panes":   [PaneInfo, ...]
}
```

Three flat lists. `PaneInfo.link_id` + `PaneInfo.window_id` carry the
associations. The v0.2.0 nested `Vec<LinkSnapshot>` schema is GONE.
There is NO migration — users must `rm ~/.config/xscore/state/state.json`
on upgrade (documented in CHANGELOG).

### `?tmux=...` semantics (unchanged from v0.2.0)

- `?tmux=off` or absent → `RawSink` (1 active pane at a time, Q1)
- `?tmux=on|auto|local|remote` → `TmuxSink` (multi-pane capable)

`SinkConfig` enum unchanged from v0.2.0 (still `Raw` | `Tmux`).

### Pane-add → Pane-bridge mapping

When a pane is created via `pane add`, it's an orphan (no link, no
window). To get data I/O:

```bash
xscore pane add --cols 80 --rows 24
# pane:  01928e0a-...

xscore link add shell:///bin/cat
# link:  01928e0a-...

xscore pane attach --pane 01928e0a-pane --link 01928e0a-link
# attached: 01928e0a-pane → link 01928e0a-link

xscore data write --pane 01928e0a-pane <<< 'echo hi'
xscore data read --pane 01928e0a-pane
```

The pane can move freely between windows and between links:

```bash
xscore pane add-to-window --pane <pid> --window <wid>
xscore pane detach --pane <pid>            # buffer preserved
xscore pane attach --pane <pid> --link <other-lid>
```

### Q1 again — Q1 corollary (multi-pane for tmux)

Tmux-backed links (`?tmux=on|auto|local|remote`) may host N simultaneous
panes — each `attach_pane` creates a new `tmux split-window` in the
link's tmux session. No auto-detach.

## Acceptance

- All commits implementing this ADR land before the `v0.3.0` tag.
- `cargo check -p xscore --tests` is clean.
- `cargo test -p xscore -- --test-threads=1` passes (target ~30-38 tests).
- ADR 0001, 0002, 0003, 0006 each get a "Revised 2026-10-19/20" note
  pointing here.
- `doc/dev/roadmap/overview.md` reflects the 3-entity decoupled model.
- README 5-min demo shows the new flow (`add → add → attach → data`).
- CHANGELOG has `[0.3.0]` entry documenting the BREAKING redesign.

## Migration from v0.2.0

NO automatic migration. The state.json schema is incompatible.

```bash
# v0.2.0 → v0.3.0: clear state and re-add everything
$ rm ~/.config/xscore/state/state.json
$ xscore link add shell:///bin/cat
# ... re-add links, panes, windows as needed
```

Documented in CHANGELOG.md `[0.3.0] - 2026-10-20` section.

## Consequences

### Positive

- Clean separation of concerns: transport vs UI vs buffer.
- Panes are reusable buffers — switch links without losing data.
- `xscore pane add` + `xscore pane attach` is composable (no
  auto-window-pane side effects).
- Orphan panes are testable in isolation.
- Direct-bridge ring buffer (v0.4.0+) becomes straightforward to add.

### Negative

- **State schema BREAKING** — users must delete `state.json` on upgrade.
- CLI verb count grew (new `pane attach/detach/add-to-window/...`).
- `link del` cascades to panes (silent detach) — previously panes
  were owned by windows (now panes are independent entities that
  lose their link_id on link deletion; panes themselves survive).
- Hydration no longer auto-builds I/O — users must `pane attach`
  after restart (previously `link open` did this for tmux).

### Alternatives considered

**A. Keep 3-tier Link → Window → Pane hierarchy (v0.2.0).**
Rejected: user explicitly said "建立link不要自动生成window或者pane"
and "pane可以自由的切换link" — the 3-tier hierarchy implies fixed
ownership.

**B. Single flat entity (no Window at all).**
Considered: every pane is its own window. Rejected: a Window gives
users a way to group panes (tabs, splits, grid) — pure UI value.

**C. `PaneInfo.link_id` as `Vec<LinkId>` (multi-link).**
Rejected: the user's Q1 rule is "ONE active link per pane at a time".
A pane can have history (via re-attach) but is currently attached to
at most one link.

## Notes

- Bridge storage is separated from `PaneInfo` because mutating info
  (in `PaneRegistry`) would otherwise destroy the bridge. See
  `SessionManager::pane_bridges`.
- `link.active_pane: Mutex<Option<PaneId>>` is a CACHE for direct
  links; the source of truth for Q1 is pane info (persisted).
- `Arc<dyn Transport>` (was `Box<dyn Transport>` in v0.2.0) is required
  so `DirectBridge { transport: Arc<dyn Transport> }` can hold a
  shared reference.