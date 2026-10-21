# Architecture Diagrams



## 1. Link × Window × Pane topology

Runtime structure: CLI surface, `SessionManager`, three registries, the
`Link` / `Pane` structs, `Transport` × `Sink` trait impls, and external
processes (tmux session inside shared `-L xscore`, local child, serial port, future SSH channel).
Color-coded by layer.

Read first if you want to know "what are the moving parts".

```mermaid
graph TB
    subgraph CLI ["CLI (clap)"]
        Connect["xscore link add<br/>-c CONFIG"]
        LinkCmd["xscore link<br/>add/del/open/close<br/>show/list/config {add,remove,show,list}"]
        WindowCmd["xscore window<br/>add/del/show/list (ADR 0006)"]
        PaneCmd["xscore pane<br/>add/del/show/list"]
        DataCmd["xscore data<br/>read/write/resize<br/>--pane"]
        ConfigCmd["xscore config<br/>show/path/set/unset"]
    end

    subgraph Core ["core::SessionManager"]
        SM["SessionManager"]
        LR["LinkRegistry<br/>{ by_id, by_address }"]
        WR["WindowRegistry<br/>{ by_id, by_link } (ADR 0006)"]
        PR["PaneRegistry<br/>{ PaneInfo.window_id } (ADR 0006: was link_id)"]
        BR["BackendRegistry"]
    end

    subgraph LinkModel ["Link × Window × Pane model (ADR 0006)"]
        Link["Link {<br/>  id: LinkId<br/>  info: LinkInfo<br/>  transport: Box&lt;dyn Transport&gt;<br/>  sink: Box&lt;dyn Sink&gt;<br/>  windows: Mutex&lt;Vec&lt;WindowId&gt;&gt;<br/>} (ADR 0006: was panes)"]
        Window["WindowInfo {<br/>  id: WindowId<br/>  link_id: LinkId<br/>  name: String<br/>  tmux_window_index: u32<br/>}"]
        Pane["PaneInfo {<br/>  id: PaneId<br/>  window_id: WindowId<br/>  sink_handle: String<br/>  pane_index: u32<br/>} (ADR 0006: was link_id)"]
        PB["PaneBackend (composite)<br/>{ (Link, Window, sink_handle) }"]
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
    WindowCmd --> SM
    PaneCmd --> SM
    DataCmd --> PB
    SM --> LR
    SM --> WR
    SM --> PR
    SM --> BR
    LR --> Link
    WR --> Window
    PR --> Pane
    BR --> PB
    Link --> TransportLayer
    Link --> SinkLayer
    Link --> Window
    Window --> Pane
    PB --> Link
    PB --> Pane
    PB --> Window

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

    class Connect,LinkCmd,WindowCmd,PaneCmd,DataCmd,ConfigCmd cli
    class SM,LR,WR,PR,BR core
    class Link,Window,Pane,PB model
    class TS,TT,TU,TIF,ST,SR,SIF layer
    class TMUX,Shell,Serial,SSH external
```

## 2. Link × Window × Pane state machines

Three parallel state diagrams (ADR 0006).

- **Link lifecycle**: `Idle → Constructing → Open → Ready → Closing → Closed` for the **resource**
  (tmux session stays alive across `Closing` — i.e. `close` is tmux-detach). For full teardown the path
  is `Ready → Deleting → Closed` (deletes the per-link tmux session; cascades every window + pane).
  A sub-machine `Active (N windows)` for the steady state (`NoWindow ↔ OneWindow ↔ ManyWindows`).
- **Window lifecycle**: `Created → Active → Closing → Closed` for the **tmux window** inside the link's
  session. With its own `Active (M panes)` sub-machine.
- **Pane lifecycle**: `Created → Open → ReadWrite ↔ Closing → Closed`. The leaf — no further sub-machine.

Key invariants (ADR 0005 verb split + ADR 0006 3-tier model):

- `close_link()` (which is `xscore link close`) is **tmux-detach only** — unregisters the link + every window + every pane in-memory; the tmux session + windows stay alive on the tmux server. Re-attach with `link open` (same session comes back, panes re-discovered).
- `del_link()` (which is `xscore link del`) is **full teardown** — kills the per-link tmux session (cascades every window + pane). The shared `tmux -L xscore` server stays up.
- `window del()` kills one tmux window; panes inside die with it (`tmux kill-window`).
- `pane del()` kills one tmux split-window; the parent window and link are unaffected.
- `link add` auto-creates one default window (named `w-<first 8 hex chars of link_id>`, e.g. `w-01928e0a`) and one default pane inside it — common path stays one step.
- Pane close ≠ Window close ≠ Link close (each level is independent).
- Failed is reachable from any transport / sink / tmux error; cleanup is best-effort.

```mermaid
stateDiagram-v2
    [*] --> Idle

    state "Link lifecycle" as LL {
        [*] --> Constructing
        Constructing --> Open: transport.open() ok
        Constructing --> Failed: transport.open() err
        Open --> Ready: sink_for().attach() ok
        Open --> Failed: sink err

        Ready --> Closing: close_link()        # ADR 0005: tmux-detach only
        Closing --> Ready: link open          # ADR 0006: re-attach (tmux session + windows stay alive)
        Ready --> Deleting: del_link()         # ADR 0005/0006: cascade every window + pane
        Deleting --> Closed: tmux kill-session + transport.close()
        Failed --> Closed: best-effort cleanup

        state "Active (N windows)" as Active {
            [*] --> NoWindow
            NoWindow --> OneWindow: link add (auto default window) OR window add
            OneWindow --> ManyWindows: window add
            ManyWindows --> OneWindow: window del
            OneWindow --> NoWindow: window del (last default)
        }
    }

    state "Window lifecycle (one per window; canonical shape)" as WL {
        [*] --> Created
        Created --> Active: sink.new_window() (named, returns tmux_window_index)
        Active --> Closing: window del
        Closing --> Closed: tmux kill-window ok
        Active --> Failed: kill-window err
        Failed --> Closed: best-effort cleanup

        state "Window.Active (M panes)" as WindowActive {
            [*] --> NoPane
            NoPane --> OnePane: pane add (split-window) #1
            OnePane --> ManyPanes: pane add #N
            ManyPanes --> OnePane: pane del
            OnePane --> NoPane: pane del (last)
        }
    }

    state "Pane lifecycle (leaf)" as PL {
        [*] --> Created
        Created --> Open: backend.insert() / tmux split-window
        Open --> ReadWrite: snapshot/write ok
        ReadWrite --> ReadWrite: read/write/resize
        ReadWrite --> Closing: pane del
        Closing --> Closed: tmux kill-split-pane ok
        Closed --> [*]
    }

    Idle --> Constructing: xscore link add (auto 1 default window + 1 pane per ADR 0006)
    Constructing --> Ready: success
    Closed --> [*]

    note right of Active
        Link hosts N windows;
        each window hosts M panes.
        3-tier model (Link / Window / Pane)
        per ADR 0006.
        Pane close ≠ Window close ≠ Link close.
    end note

    note right of Failed
        Reachable from any
        transport / sink / tmux error.
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