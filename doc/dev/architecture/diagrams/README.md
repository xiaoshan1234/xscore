# Architecture Diagrams

Inline Mermaid diagrams for the xscore architecture. Each block below renders
in any Markdown preview that supports Mermaid (GitHub, GitLab, IDE preview,
Obsidian, etc.), or paste into https://mermaid.live for an instant check.

If you need a standalone SVG (e.g. for a slide deck), paste a diagram into
https://mermaid.live and use its export panel, or run any Mermaid CLI
(e.g. `npm i -g @mermaid-js/mermaid-cli`) against the diagram body. The
`mermaid` fenced blocks here are the source of truth — there are no
separate source files to keep in sync.

## 1. Link × Pane topology

Runtime structure: CLI surface, `SessionManager`, three registries, the
`Link` / `Pane` structs, `Transport` × `Sink` trait impls, and external
processes (tmux server, local child, serial port, future SSH channel).
Color-coded by layer.

Read first if you want to know "what are the moving parts".

```mermaid
graph TB
    subgraph CLI ["CLI (clap)"]
        Connect["xscore link add<br/>-c CONFIG"]
        LinkCmd["xscore link<br/>add/open/close/remove<br/>show/list/config {add,remove,show,list}"]
        PaneCmd["xscore pane<br/>add/open/close/remove<br/>show/list"]
        DataCmd["xscore data<br/>read/write/resize<br/>--pane"]
        ConfigCmd["xscore config<br/>show/path/set/unset"]
    end

    subgraph Core ["core::SessionManager"]
        SM["SessionManager"]
        LR["LinkRegistry<br/>{ by_id, by_address }"]
        PR["PaneRegistry"]
        BR["BackendRegistry"]
    end

    subgraph LinkModel ["Link × Pane model"]
        Link["Link {<br/>  id: LinkId<br/>  info: LinkInfo<br/>  transport: Box&lt;dyn Transport&gt;<br/>  sink: Box&lt;dyn Sink&gt;<br/>  panes: Mutex&lt;Vec&lt;PaneId&gt;&gt;<br/>}"]
        Pane["PaneInfo {<br/>  id: PaneId<br/>  link_id: LinkId<br/>  sink_handle: String<br/>  pane_index: u32<br/>}"]
        PB["PaneBackend (composite)<br/>{ (Link, sink_handle) }"]
    end

    subgraph TransportLayer ["transport/"]
        TS["ShellTransport"]
        TT["SerialTransport<br/>(stub)"]
        TU["SshTransport<br/>(stub)"]
        TIF["transport_for()"]
    end

    subgraph SinkLayer ["sink/"]
        ST["TmuxSink<br/>(multi-pane)"]
        SR["RawSink (stub)"]
        SIF["sink_for()"]
    end

    subgraph External ["External"]
        TMUX["tmux -L xscore<br/>(external server)"]
        Shell["local child"]
        Serial["serial port"]
        SSH["SSH channel<br/>(future)"]
    end

    Connect --> SM
    LinkCmd --> SM
    PaneCmd --> SM
    DataCmd --> PB
    SM --> LR
    SM --> PR
    SM --> BR
    LR --> Link
    PR --> Pane
    BR --> PB
    Link --> TransportLayer
    Link --> SinkLayer
    PB --> Link
    PB --> Pane

    TIF --> TS
    TIF --> TT
    TIF --> TU
    TS --> Shell
    TT --> Serial
    TU -.->|future| SSH

    SIF --> ST
    SIF --> SR
    ST --> TMUX

    classDef cli fill:#e3f2fd,stroke:#1976d2
    classDef core fill:#fff3e0,stroke:#f57c00
    classDef model fill:#f3e5f5,stroke:#7b1fa2
    classDef layer fill:#e8f5e9,stroke:#388e3c
    classDef external fill:#fafafa,stroke:#616161,stroke-dasharray: 5 5

    class Connect,LinkCmd,PaneCmd,DataCmd,ConfigCmd cli
    class SM,LR,PR,BR core
    class Link,Pane,PB model
    class TS,TT,TU,TIF,ST,SR,SIF layer
    class TMUX,Shell,Serial,SSH external
```

## 2. Link & Pane state machines

Two parallel state diagrams.

- **Link lifecycle**: `Idle → Constructing → Open → Ready → Closing → Closed`,
  with a sub-machine `Active (N panes)` for the steady state
  (`Empty ↔ OnePane ↔ ManyPanes`).
- **Pane lifecycle**: `Created → Open → ReadWrite ↔ Closing → Closed`.

Key invariants:

- Link stays Alive across pane lifecycle (Pane close ≠ Link close).
- Failed state is reachable from any transport / sink error; cleanup is
  best-effort.

```mermaid
stateDiagram-v2
    [*] --> Idle

    state "Link lifecycle" as LL {
        [*] --> Constructing
        Constructing --> Open: transport.open() ok
        Constructing --> Failed: transport.open() err
        Open --> Ready: sink_for().attach() ok
        Open --> Failed: sink err
        Ready --> Closing: close_link()
        Closing --> Closed: transport.close() ok
        Failed --> Closed: best-effort cleanup

        state "Active (N panes)" as Active {
            [*] --> Empty
            Empty --> OnePane: new_pane() #1
            OnePane --> ManyPanes: new_pane() #N
            ManyPanes --> OnePane: close_pane()
            OnePane --> Empty: close_pane() last
        }
    }

    state "Pane lifecycle" as PL {
        [*] --> Created
        Created --> Open: backend.insert()
        Open --> ReadWrite: snapshot/write ok
        ReadWrite --> ReadWrite: read/write/resize
        ReadWrite --> Closing: close_pane()
        Closing --> Closed: sink.close_pane() ok
    }

    Idle --> Constructing: xscore link add
    Constructing --> Ready: success
    Ready --> Active
    Ready --> Closing: close_link
    Closed --> [*]

    note right of Active
        Link stays Alive
        across pane lifecycle.
        Pane close ≠ Link close.
    end note

    note right of Failed
        Reachable from any
        transport / sink error.
        Recovery: best-effort
        cleanup, then Closed.
    end note
```

## 3. Module dependency rules

Directed graph of allowed dependencies between top-level modules
(`cli`, `core`, `model`, `url`, `transport`, `sink`, `backends`, `config`,
`util`). Solid arrows = allowed. Dashed red X arrows = forbidden
(e.g. `cli` cannot import `transport` directly).

This is the "hard rules" section of [`overview.md`](../../roadmap/overview.md)
rendered as a graph.

```mermaid
graph LR
    subgraph allowed_deps ["allowed dependencies (Module rules)"]
        CLI["cli/*"]
        CORE["core/*"]
        MODEL["model/*"]
        URL["url/*"]
        TRANSPORT["transport/*"]
        SINK["sink/*"]
        BACKENDS["backends/*"]
        CONFIG["config/*"]
        UTIL["util/*"]
    end

    CLI --> CORE
    CLI --> MODEL
    CLI --> URL
    CLI --> CONFIG

    CORE --> MODEL
    CORE --> URL
    CORE --> BACKENDS
    CORE -.uses.-> TRANSPORT
    CORE -.uses.-> SINK

    BACKENDS --> TRANSPORT
    BACKENDS --> SINK
    BACKENDS --> MODEL
    BACKENDS --> CORE

    TRANSPORT --> MODEL
    TRANSPORT --> CORE

    SINK --> MODEL
    SINK --> CORE
    SINK --> TRANSPORT

    URL --> MODEL

    CONFIG --> MODEL

    UTIL -.used by.-> SINK
    UTIL -.used by.-> TRANSPORT

    classDef forbiddenEdge stroke:#c62828,stroke-width:2px,stroke-dasharray: 5 5
    linkStyle 11,12,13,14,15,16,17,18,19,20,21 stroke:#c62828,stroke-width:2px,stroke-dasharray: 5 5

    %% Forbidden edges (Module rules violations):
    CLI -->|FORBIDDEN| TRANSPORT
    CLI -->|FORBIDDEN| SINK
    CLI -->|FORBIDDEN| BACKENDS

    TRANSPORT -->|FORBIDDEN| CLI
    SINK -->|FORBIDDEN| CLI
    BACKENDS -->|FORBIDDEN| CLI

    MODEL -->|FORBIDDEN| CORE
    MODEL -->|FORBIDDEN| SINK

    CONFIG -->|FORBIDDEN| CORE

    URL -->|FORBIDDEN| CORE
    URL -->|FORBIDDEN| TRANSPORT
```

## Cross-references

- [`overview.md`](../../roadmap/overview.md) — text version of the architecture
- [ADR 0001](../../adr/0001-link-pane-model.md) — Link × Pane model rationale
- [ADR 0002](../../adr/0002-transport-sink-split.md) — Transport × Sink split rationale
- [ADR 0003](../../adr/0003-external-tmux-server.md) — External tmux server rationale
- [ADR 0004](../../adr/0004-link-existing-dedup.md) — `--link=existing` semantics rationale

## Updating

When the architecture changes:

1. Edit the `mermaid` block directly in this file.
2. If the change is structural (new trait, new registry, new module), update
   both `overview.md` and the relevant ADR **before** touching the diagram.
3. Don't let a diagram drift from `overview.md`. The text and the diagram
   are a pair — they describe the same architecture, and drift between them
   is a bug, not a stylistic difference.
