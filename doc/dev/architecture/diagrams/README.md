# Architecture Diagrams

Source files are Mermaid (`.mmd`). Each one is renderable independently:

```bash
# Render one to SVG (requires mermaid-cli: npm i -g @mermaid-js/mermaid-cli)
mmdc -i link-pane-topology.mmd -o link-pane-topology.svg
mmdc -i lifecycle.mmd           -o lifecycle.svg
mmdc -i module-deps.mmd        -o module-deps.svg

# Or paste into https://mermaid.live for instant preview.
```

## Diagrams

### 1. `link-pane-topology.mmd` — Link × Pane topology

Shows the runtime structure: CLI surface, `SessionManager`, three registries, the Link/Pane structs, Transport × Sink trait impls, and external processes (tmux server, local child, serial port, future SSH channel). Color-coded by layer.

Read first if you want to know "what are the moving parts".

### 2. `lifecycle.mmd` — Link & Pane state machines

Two parallel state diagrams:

- **Link lifecycle**: `Idle → Constructing → Open → Ready → Closing → Closed`, with a sub-machine `Active (N panes)` for the steady state (Empty ↔ OnePane ↔ ManyPanes).
- **Pane lifecycle**: `Created → Open → ReadWrite ↔ Closing → Closed`.

Key invariants called out in notes:
- Link stays Alive across pane lifecycle (Pane close ≠ Link close).
- Failed state is reachable from any transport / sink error; cleanup is best-effort.

### 3. `module-deps.mmd` — Module dependency rules

Directed graph of allowed dependencies between top-level modules (`cli`, `core`, `model`, `url`, `transport`, `sink`, `backends`, `config`, `util`). Solid arrows = allowed. Dashed red X arrows = forbidden (e.g. `cli` cannot import `transport` directly).

This is the "hard rules" section from `overview.md` rendered as a graph.

## Cross-references

- [overview.md](../../roadmap/overview.md) — text version of the architecture
- [ADR 0001](../../adr/0001-link-pane-model.md) — Link × Pane model rationale
- [ADR 0002](../../adr/0002-transport-sink-split.md) — Transport × Sink split rationale
- [ADR 0003](../../adr/0003-external-tmux-server.md) — External tmux server rationale
- [ADR 0004](../../adr/0004-link-existing-dedup.md) — `--link=existing` semantics rationale

## Updating

When the architecture changes:

1. Edit the `.mmd` source.
2. Re-render the `.svg` (or update the README to skip rendering — SVG is optional).
3. If the change is structural (new trait, new registry, new module), update both `overview.md` and the relevant ADR before touching the diagram.

Don't commit a `link-pane-topology.mmd` that contradicts `overview.md`. The two are a pair.