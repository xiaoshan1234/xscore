# ADR 0007: Default sink is `raw`; tmux is opt-in via `?tmux=...`

- **Status:** Accepted
- **Date:** 2026-10-19
- **Deciders:** dev
- **Related:** [ADR 0001](./0001-link-pane-model.md) (Link × Window × Pane now applies to tmux-backed links only), [ADR 0002](./0002-transport-sink-split.md) (Sink is now optional; RawSink is the no-sink marker), [ADR 0003](./0003-external-tmux-server.md) (revised — the shared `tmux -L xscore` server is now only used when `?tmux=...` is set), [ADR 0006](./0006-window-concept.md) (revised — the 3-tier model applies to tmux-backed links only; raw links are 1-tier Link = Transport).

## Context

Pre-ADR 0007, every `xscore link add <url>` created a tmux session in the
shared `tmux -L xscore` server (per ADR 0003), and auto-created one default
window + one default pane inside it (per ADR 0006). The Link × Window × Pane
model mapped 1:1 onto tmux's session → window → pane.

This was the v0.1.0 default (released 2026-10-18). It came with two real
costs that the user flagged immediately after tagging v0.1.0:

1. **Tmux is overhead for users who don't want it.** Most users just want
   `xscore ssh h:22` or `xscore shell /bin/bash` to give them a clean CLI
   for SSH / shell / serial I/O. Forcing them to also have a tmux server
   in the loop — with all the implications (per-char `send-keys` perf, the
   race between `split-window` returning and the server committing state,
   the need to manage session lifecycle, the requirement to install tmux
   at all) — was over-engineering for the common case.

2. **The "every link goes through tmux" model hid the simpler mental
   model.** When a user types `xscore link add shell:///bin/bash`, they
   expect "a bash process." The actual answer (until this ADR) was "a
   bash process running inside a tmux window inside a tmux session inside
   the shared `tmux -L xscore` server." That's a lot of layers to explain
   for what's conceptually "run bash, give me an I/O handle."

The user reported this on 2026-10-19 (verbatim):

> "每个 link 都走 tmux server。修改这个设计 —— 直接走原生就行。
> 我本来的目的是支持 tmux 会话的管理，不是所有 link 都要走 tmux。"

Translation: "Every link goes through tmux server. Change this design —
just use native [transport I/O] is fine. My original purpose was to
support tmux session management, not that all links have to go through
tmux."

## Decision

**Default behavior:** `xscore link add <url>` runs the transport directly
with no tmux server involved. The link's sink is `RawSink` (a no-op marker
in the `Sink` trait). Only `link: <id>` is printed.

**Opt-in to tmux:** Use the `?tmux=` URL query:

| `?tmux=` value | Sink kind | Window/Pane tree | Data ops |
|---|---|---|---|
| _absent_ (default) | `raw` | no | (no panes) |
| `?tmux=off` | `raw` | no | (no panes) |
| `?tmux=on` | `tmux` (mode=auto) | yes | yes |
| `?tmux=auto` | `tmux` (mode=auto) | yes | yes |
| `?tmux=local` | `tmux` (mode=local) | yes | yes |
| `?tmux=remote` | `tmux` (mode=remote) | yes | yes |

`auto` resolves at link-creation time: `ssh://` → `remote` (tmux on the
remote host), `shell://` / `serial://` → `local` (the local shared
`tmux -L xscore` server). `name=X` overrides the auto-generated
`xscore-<link_id>` tmux session name; only meaningful with `?tmux=...`.

### Schema change

`SinkConfig` enum gains a `Raw` variant:

```rust
pub enum SinkConfig {
    /// Direct transport I/O. No tmux server involvement.
    /// The shell/serial/ssh transport is the I/O source directly.
    Raw,
    /// Wrap the link in a tmux session (the shared `tmux -L xscore`
    /// server). Opt in via `?tmux=on|auto|local|remote`.
    Tmux {
        mode: String,             // "auto" | "local" | "remote"
        session_name: Option<String>,
    },
}
```

State files written by v0.1.0 (`SinkConfig::Tmux { ... }` only) deserialize
unchanged — the `Tmux` variant is still in the enum, `Raw` was added
alongside. New raw links serialize as `"sink": "raw"`.

### Manager method changes

| Method | Old return type | New return type |
|---|---|---|
| `add_link` | `(LinkId, WindowId, PaneId)` | `(LinkId, Option<WindowId>, Option<PaneId>)` |
| `open_link` | `(LinkId, WindowId, PaneId)` | `(LinkId, Option<WindowId>, Option<PaneId>)` |

For raw links, `add_link` returns `(link_id, None, None)` and skips the
auto-create window + pane. For tmux links, behavior is unchanged.

`add_window` and `add_pane` now return a new error variant when called
on a raw link:

```rust
Error::NotTmuxLink(LinkId)
// displays as: "link <id> does not use the tmux sink;
//               add `?tmux=on` to the URL to enable window/pane commands"
```

This is checked BEFORE the `unreachable!()` in `RawSink::new_window` /
`new_pane` would otherwise panic.

`hydrate` (state persistence) now branches on `SinkConfig`:
- Raw links adopt with a `RawSink`; the `tmux has-session` check is
  skipped (no session to check).
- Tmux links keep the existing verify-and-adopt path.

### CLI behavior changes

- `xscore link add <url>` (default): prints only `link: <id>`.
- `xscore link add <url>?tmux=on`: prints `link:`, `window:`, `pane:`
  (the v0.1.0 behavior).
- `xscore window add --link <raw-link-id>`: errors with `NotTmuxLink`.
- `xscore window list`, `pane list`, `data {read,write,resize}`: all
  return `NotTmuxLink` for raw links.

## Consequences

**Positive:**
- Most users get the simpler mental model: "a link is a transport." No
  tmux server in the loop, no install requirement, no `split-window` race
  to debug.
- The full Link × Window × Pane model is still available for users who
  want it (just add `?tmux=on`).
- The `Sink` trait is now a true "optional layer" — `RawSink` is a no-op
  marker, `TmuxSink` is the real implementation. Cleaner separation of
  concerns.
- Tmux race bugs (the `tmux split-window` server-internal async race
  that caused the v0.1.0 `data read` failure) only matter for tmux
  users now. Raw users never hit them.

**Negative:**
- **Breaking behavior change vs v0.1.0.** Any user (or test) relying on
  the v0.1.0 default — every `link add` creates a tmux session with a
  default window + pane — now gets a raw link instead. The fix is one
  URL change (`?tmux=on` or `?tmux=auto`), but it's a real break.
- `xscore window list` on a raw link is empty (no windows). `xscore
  pane list` is empty. `xscore data {read,write,resize}` errors. Users
  may not realize why their usual flow stopped working until they read
  the README.
- Version bump required: v0.1.0 → v0.2.0. The CHANGELOG [Unreleased]
  section flags the breaking change prominently.
- The `add_link` return type change is a real API change. CLI tests
  that did `let (lid, wid, pid) = mgr.add_link(...)` need to handle
  `Option<WindowId>` and `Option<PaneId>` now. (Updated in the same
  commit.)

**Risks:**
- Users on v0.1.0 with a populated `~/.config/xscore/state/state.json`
  whose links are all `Tmux` will see no behavior change (Tmux variant
  unchanged, hydration still verifies the session). But new links
  created after the upgrade default to raw. Mixing tmux + raw links
  in the same state is fine; the manager dispatches per-link on sink
  type.
- The `?tmux=on` default-mode `auto` for ssh is "remote tmux" — this
  requires tmux to be installed on the remote host. If the remote
  doesn't have tmux, the user has to either install it or use
  `?tmux=off` and live with no window/pane tree for the SSH link.
  Documented in the README.

## Alternatives considered

- **Keep tmux as the default, add a `--raw` flag to opt out.**
  Rejected. The user explicitly asked for the inverse: "directly use
  native is fine" — i.e. they want the simpler behavior by default and
  tmux as the opt-in. Flipping the default is a one-line URL change
  for users; the alternative (add a flag) is more code, more docs, and
  the wrong default for the common case.
- **Three modes: raw / tmux-auto / no-tmux-with-panes.** Rejected.
  Over-engineering. "Raw" means "no panes"; "tmux" means "tmux with
  the standard window/pane tree." That's the binary distinction that
  matters.
- **Make `link add` always create a tmux session but mark it as
  "background" — no default window/pane unless requested.** Rejected.
  Same outcome (link without windows) but goes through tmux for no
  reason. The user wants the tmux server out of the loop.
- **Drop tmux entirely, implement a Rust-native PTY/ANSI parser.**
  Rejected. This is the original rejected alternative in ADR 0003 —
  it's the work the tmux server was chosen to avoid. Reverting is a
  massive implementation cost, and tmux is still useful for users
  who want it (session persistence, multi-window, attach from
  outside xscore).
- **Per-URL opt-in to a simpler two-tier model (Link → Pane, no
  Window).** Rejected. The 3-tier model works fine for tmux users
  who opt in; the simpler 1-tier (Link = transport) is what raw users
  get. No need for a 2-tier middle ground.

## Migration from v0.1.0

1. **State files:** no migration needed. `SinkConfig::Tmux` variant
   is unchanged.
2. **CLI scripts:** add `?tmux=auto` to URLs that need the window/pane
   tree. Find+replace: `shell:///bin/bash` → `shell:///bin/bash?tmux=auto`
   (or wherever the user had a tmux-backed link).
3. **Tests:** any `xscore link add` test that expects `window:` /
   `pane:` lines in stdout must add `?tmux=...` to the URL.
   Updated in commit `efccc6b` (the 4 cli_smoke + 3 data_smoke URLs
   that previously expected tmux behavior).
4. **API users:** `add_link` / `open_link` now return
   `Option<WindowId>` / `Option<PaneId>`. Handle the None case for
   raw links (no auto-created window/pane).

## Acceptance

This ADR is **Accepted** on the date above. The implementing commit is
`efccc6b` ("feat(core)!: tmux is opt-in via `?tmux=...`, default sink is
now raw"). The 9 affected files are listed in that commit's message.

Companion updates to keep the doc tree consistent:
- ADR 0003 — add a "Revised 2026-10-19" subsection noting that the
  shared `tmux -L xscore` server is now only used for tmux-backed
  links. The original decision ("every link is a tmux session inside
  the shared server") is now the opt-in path.
- ADR 0006 — add a "Revised 2026-10-19" subsection noting that the
  3-tier Link × Window × Pane model applies to tmux-backed links only.
  Raw links are 1-tier (Link = Transport).
- ADR 0001 — minor note that Link × Window × Pane is the model for
  tmux-backed links; the addressable model is 1-tier for raw links.
- ADR 0002 — note that `Sink` is now optional; `RawSink` is the
  no-sink marker for raw links.
- `doc/dev/roadmap/overview.md` — update the architecture diagram
  and the URL examples to show the new default and the `?tmux=...`
  opt-in.
- `README.md` — already updated in commit `efccc6b` (5-min demo +
  "Sink options (tmux opt-in)" section).
- `CHANGELOG.md` — already updated in commit `efccc6b`
  ([Unreleased] → "Changed" + "Added" sections).
