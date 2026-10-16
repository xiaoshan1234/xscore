# Archive — 2026-10-16 — ADR 0004 reversal (`--link=existing` retired)

ADR 0004 introduced `--link=existing` as an opt-in flag on `xscore link add` to
attach a new pane to an existing link matched by `LinkAddress`. The user
reviewed and rejected it on the second iteration of the `link` CLI.

## Why archived

`--link=existing` conflated two distinct intents ("create a new resource" vs
"re-attach an existing one") behind one verb. The rejection surface
(`Error::LinkExistingUnsupported` for shell/serial; not-yet-implemented for
ssh) showed the abstraction didn't fit the transports. Tmux was the only
transport where attach-by-name is meaningful; the other transports have no
cross-invocation identity to attach to.

## What replaces it

The replacement is **ADR 0005: `link` verbs are `add` / `del` / `open` /
`close` (no `--link=existing`)** — explicit verb semantics instead of a
single verb with an opt-in flag:

- `link add` — always create
- `link open` — always attach (tmux only)
- `link close` — tmux-detach only
- `link del` — full teardown (was old `link close`)

The hashable `LinkAddress` that ADR 0004 defined as the user-facing dedup
key is now an internal-only concept inside `LinkRegistry`.

## Files

- `0004-link-existing-dedup.md` — the original ADR (status: Accepted →
  Superseded)
- Active replacement: [ADR 0005](../../0005-add-open-close-del-verbs.md)

## Active references that were updated to point at ADR 0005

- `../../roadmap/overview.md` — ADR list
- `../../roadmap/README.md` — task table mentions of ADR 0004
- `../../architecture/diagrams/README.md` — Cross-references
- `../../roadmap/task-10-cli-connect-link-pane.md` — the smoke / commit /

## Reference

- Superseded by: `../../0005-add-open-close-del-verbs.md`
- Decision rationale: ADR 0005 §Context and §Decision
- Original decision (now reversed): `0004-link-existing-dedup.md`
