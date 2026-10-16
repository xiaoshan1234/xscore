# Archive — 2026-10-16 — diagram `.mmd` sources

These three `.mmd` files were originally committed on 2026-10-15 in
`533aa2a docs(architecture): ADRs for the 4 key decisions + 3 Mermaid diagrams`.

## Why archived

The architecture diagrams are now rendered **inline as Mermaid fenced code
blocks** in
[`../architecture/diagrams/README.md`](../architecture/diagrams/README.md).
Keeping the diagrams as separate `.mmd` files in addition to the inline
blocks created redundant drift surfaces (text updated in README, source not
updated, etc.). The `.mmd` files were the original source-of-truth and are
preserved here in case a clean downstream diff is needed later.

## Files

- `link-pane-topology.mmd` — runtime structure: CLI / `SessionManager` /
  three registries / `Link` / `Pane` / transports / sinks / externals.
- `lifecycle.mmd` — Link and Pane state machines (includes the
  `Active (N panes)` sub-machine and the link-keeps-alive invariant notes).
- `module-deps.mmd` — allowed vs. forbidden module dependencies
  (the "hard rules" rendered as a graph).

## What to do if you need them

- **If the inline diagrams in `architecture/diagrams/README.md` and these
  archived `.mmd` files drift,** the inline version is canonical (it's what
  every reader sees). Re-derive from the inline block.
- **If you want a fresh `.mmd` source for some reason** (e.g. a CI step that
  parses Mermaid externally), extract each `mermaid` fenced block from
  `architecture/diagrams/README.md` and write it to its own `.mmd` file —
  the blocks are byte-identical to what was archived here.

## Reference

- Inline version: `../architecture/diagrams/README.md`
- Decision rationale: the same `533aa2a` commit message ("3 Mermaid diagrams")
  still describes what each diagram shows; the prose annotations there were
  folded into the README when the diagrams were embedded.
