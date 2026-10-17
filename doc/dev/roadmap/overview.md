# xscore CLI First — Architecture Overview

**Goal:** Build the CLI-first slice of xscore — a Rust workspace that manages SSH / shell / serial sessions behind a single `Link × Pane` abstraction, callable from `xscore` CLI today and (later) from MCP server and IPC server without code rewrite.

**Architecture Decision Records (read first, in order):**
- [ADR 0001](../adr/0001-link-pane-model.md) — why Link × Pane (not flat Session)
- [ADR 0002](../adr/0002-transport-sink-split.md) — why Transport × Sink (not single backend)
- [ADR 0003](../adr/0003-external-tmux-server.md) — why external `tmux -L xscore` (not embedded tmux)
- [ADR 0005](../adr/0005-add-open-close-del-verbs.md) — why `link` verbs are `add` / `del` / `open` / `close` (no `--link=existing`)

**Architecture diagrams:** [diagrams/README.md](../architecture/diagrams/README.md) (Link × Pane topology, Link & Pane state machines, Module dependency rules — inline Mermaid)

**Architecture (Link × Pane):**

A **Link** is one physical connection. A **Pane** is one logical view into a link. One link can host N panes; panes on the same link share Transport + Sink infrastructure but hold independent buffers, IDs, and lifecycle.

```
Link  ── owns ──> Transport + Sink (the physical connection + terminal semantics)
       +─ hosts ─> [Pane_1, Pane_2, ..., Pane_N]   (N independent views)
```

- **Link** = `(Transport + Sink)` pair (the same architecture we already settled on). One link is one tmux server, or one SSH channel, or one serial port, or one local child process — the physical layer.
- **Pane** = one SessionId. Each pane is what users `data read/write/resize` against. Each pane has its own window inside the link's tmux server (when sink is `TmuxSink`), or its own buffered slice (when sink is `RawSink`).

```
SessionManager
├─ LinkRegistry: link_id  -> Arc<Link>      { transport + sink + pane-list }
├─ PaneRegistry: pane_id  -> PaneInfo       { link_id, sink_handle, ... }
└─ BackendRegistry: pane_id -> Arc<dyn PaneBackend>   { wraps (Link, sink_handle) }
```

- tmux is always external, on a dedicated `xscore` socket (`tmux -L xscore ...`). One link = one tmux server name. Multiple PaneIds = multiple windows inside that server.
- Config = `~/.config/xscore/config.json` (AppConfig) + `~/.config/xscore/links/<name>.json` (persistent ConnectSpec templates) + `~/.config/xscore/state/` (reserved for future link-state persistence).

**CLI entry:** `xscore link add <url?> [-c CONFIG]` and `xscore link {del,open,close,show,list}` (ADR 0005). At least one of `<url>` / `-c CONFIG` is required. URLs:

```text
xscore link add shell:///bin/bash
xscore link add serial:///dev/ttyUSB0?baud=115200&data_bits=8
xscore link add ssh://user@host:22?identity=~/.ssh/id_ed25519&proxy=ssh://jump@1.2.3.4
xscore link add ssh://user@host:22?tmux=on    # default

# Multiple panes on the same link (ADR 0005 verb split):
xscore link add ssh://u@h:22                                            # create
xscore link add ssh://u@h:22                                            # pane 2 (different link)
# For tmux-attach instead of pane-2-on-new-link:
xscore link open 'tmux://my-session'                                    # attach existing
xscore link close --id <link-id>                                        # tmux-detach only
xscore link del --id <link-id>                                          # full teardown
```

Other commands are organized into two tiers (`link` for the physical connection, `pane` for the views). `--from spec.json` was removed in favor of `link config add/remove/show/list` templates under `~/.config/xscore/links/`.

**Link uniqueness rule (locked from user):** A link is uniquely identified by the `(transport_kind, address-tuple)`:
- `shell` → `(command, args-tuple)`
- `serial` → `(path)`
- `ssh` → `(host, port, user, identity, proxy)`

Two `xscore link add ssh://u@h:22` calls always produce **different links** (each with `link_id` = a new uuid). Re-attaching to an existing resource is a separate verb: `xscore link open <url?> [-c CONFIG]` (ADR 0005). `link open` and `link close` are tmux-only — shell/serial/ssh processes have no cross-invocation identity to attach to or detach from, and return `Error::OpenOnlyForTmux` / `Error::CloseOnlyForTmux`.

Each alive link identifies itself only via `link_id` (uuid v7) and the internal `LinkAddress` used for routing inside `LinkRegistry`. `LinkAddress` is not a user-facing dedup key (ADR 0005 retired `--link=existing`). For persistent templates use `xscore link config add <name> <url>`.

> ⚠️ **Open question (does not block the slice):** For SSH the same-address attach path needs an SSH channel + tmux-over-the-wire before it lands. Real russh wiring lands later; for now the slice delivers shell/serial links + ssh stubs (no `link open` for ssh yet — it returns an unimplemented-stub error until the SSH-integration phase resolves the remote tmux case).

**Tech Stack (locked from user):**
- CLI: `clap` v4 derive
- Async runtime: `tokio` (multi-thread, full features)
- SSH: `russh` (pure-Rust async)
- Shell: `tokio::process::Command`
- Serial: `tokio-serial`
- tmux: external `tmux` binary over CLI (`tmux -L xscore ...`)
- URL parsing: `url` crate
- Persistence: JSON files (config + per-link JSON). No SQLite this phase.
- Logging: `tracing` + `tracing-subscriber`
- Errors: `thiserror` (library) + `anyhow` (CLI top-level)
- Testing: `mockall` for trait impls + integration tests against real `tmux -L xscore-test`

**Reference:** `doc/dev/roadmap/` (this directory) — sibling to future RFCs. Task files: `task-NN-*.md`. Acceptance: `acceptance.md`.

---

## Module Boundaries (Three-Cut)

```
crates/xscore/
├── Cargo.toml
├── src/
│   ├── main.rs                       # clap entry, tracing init, dispatch
│   ├── cli/
│   │   ├── mod.rs                    # Cli struct, derive top-level
│   │   ├── link.rs                   # link add/del/open/close/show/list + config sub-tree
│   │   ├── pane.rs                   # pane add/del/show/list
│   │   ├── data.rs                   # pane data read/write/resize
│   │   └── config.rs                 # config show/path/set/unset
│   ├── core/
│   │   ├── mod.rs                    # pub SessionManager, re-exports
│   │   ├── manager.rs                # add_link / open_link / close_link / del_link; add_pane / del_pane
│   │   ├── link.rs                   # Link struct (transport + sink + pane list)
│   │   ├── pane.rs                   # Pane struct (handle + state)
│   │   ├── link_registry.rs          # LinkRegistry (by_id; address used internally for routing)
│   │   ├── pane_registry.rs          # PaneRegistry (pane_id -> PaneInfo)
│   │   ├── address.rs                # LinkAddress hashable key per transport
│   │   ├── id.rs                     # LinkId + PaneId (uuid v7)
│   │   └── error.rs                  # thiserror enum (LinkNotFound, OpenOnlyForTmux, CloseOnlyForTmux, ...)
│   ├── model/
│   │   ├── mod.rs
│   │   ├── session.rs                # ConnectSpec / LinkInfo / PaneInfo / TransportConfig / SinkConfig
│   │   └── config.rs                 # AppConfig
│   ├── url/
│   │   ├── mod.rs                    # URL parser entry
│   │   ├── scheme.rs                 # placeholder
│   │   └── connect.rs                # URL -> (ConnectSpec, LinkAddress)
│   ├── transport/
│   │   ├── mod.rs                    # pub trait Transport + address()
│   │   ├── factory.rs                # transport_for(TransportConfig) -> Box<dyn Transport>
│   │   ├── shell.rs                  # tokio::process::Command
│   │   ├── serial.rs                 # tokio-serial stub
│   │   └── ssh.rs                    # russh stub
│   ├── sink/
│   │   ├── mod.rs                    # pub trait Sink + pane ops
│   │   ├── factory.rs                # sink_for(...)
│   │   ├── tmux.rs                   # TmuxSink (PTY/ANSI), new_pane() per attach
│   │   └── raw.rs                    # RawSink (no-op stub for ?tmux=off)
│   ├── backends/
│   │   ├── mod.rs                    # pub trait PaneBackend + BackendRegistry
│   │   └── composite.rs              # PaneBackend impl for (Link, sink_handle)
│   ├── config/
│   │   ├── mod.rs
│   │   ├── loader.rs                 # load/save ConnectSpec JSON, xdg dirs
│   │   └── paths.rs                  # xdg path resolution
│   └── util/
│       ├── mod.rs
│       └── tmux_socket.rs            # socket name + helpers
└── tests/
    ├── cli_smoke.rs                  # spawn xscore binary, assert outputs
    ├── manager_mock.rs               # SessionManager with mockall Transport/Sink
    ├── url_parse.rs                  # URL -> ConnectSpec + LinkAddress
    └── tmux_integration.rs           # real tmux -L xscore-test, gated by env
```

**Module rules (hard):**
- `cli` may depend on `core`, `model`, `url`, `config`. Never on `transport` / `sink` directly.
- `core` depends on `model`, `url`, `sink::factory`, `transport::factory`, `backends`. Never on `cli` or `config`.
- `backends` depends on `transport` (trait), `sink` (trait), `model`, `core::error`. Never on `cli`.
- `transport` depends on `model`, `core::error`. Never on `sink` or `cli`.
- `sink` depends on `model`, `core::error`, `transport` (to wire at attach). Never on `cli`.
- `url` depends on `model`. Pure parsing layer, no I/O.
- `config` depends on `model`. Never on anything else.
- All cross-module state goes through `core::SessionManager` — no global statics.

**Why Link × Pane (vs single-session model):**

| Layer | Concern | Lifetime |
| --- | --- | --- |
| Transport | bytes/PTY | 1 per link; killed when link closes |
| Sink | tmux server / passthrough | 1 per link; killed when link closes |
| Pane | one working view | N per link; each pane independently closeable |

Each pane has its own `id`, its own data read/write state, its own `PaneInfo`. Killing one pane does not affect its siblings. Killing a link kills the transport + sink + every pane on it.

---

## Core Type Design (preview, refined in Tasks 2–6)

```rust
// core/id.rs
pub struct LinkId(Uuid);   // identifies a link (transport + sink + tmux server)
pub struct PaneId(Uuid);   // identifies one pane inside a link

// core/address.rs
#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub enum LinkAddress {
    Shell { command: String, args: Vec<String> },
    Serial { path: String },
    Ssh { host: String, port: u16, user: String, identity: Option<String>, proxy: Option<String> },
}

// transport/mod.rs
#[async_trait]
pub trait Transport: Send + Sync {
    fn kind(&self) -> &'static str;
    fn address(&self) -> LinkAddress;          // used for link dedup
    async fn open(&mut self) -> Result<()>;
    async fn write(&self, data: &[u8]) -> Result<usize>;
    async fn read(&self, buf: &mut [u8]) -> Result<usize>;
    async fn resize(&self, cols: u16, rows: u16) -> Result<()>;
    async fn close(&self) -> Result<()>;
}

// sink/mod.rs
#[async_trait]
pub trait Sink: Send + Sync {
    fn kind(&self) -> &'static str;
    /// Wire this sink to a transport. Called once per link lifetime.
    async fn attach(&mut self, transport: &dyn Transport) -> Result<()>;
    /// Open a new pane within this sink. Returns a sink-side handle (e.g. tmux window id "%42").
    async fn new_pane(&self) -> Result<String>;
    /// Drop one pane. Transport + sink stay alive.
    async fn close_pane(&self, pane_handle: &str) -> Result<()>;
    /// List panes — used by LinkRegistry recovery / `pane list`.
    async fn list_panes(&self) -> Result<Vec<String>>;
    /// Snapshot one pane's output.
    async fn snapshot(&self, pane_handle: &str, buf: &mut [u8]) -> Result<usize>;
    /// Write to one pane.
    async fn write(&self, pane_handle: &str, data: &[u8]) -> Result<usize>;
    /// Resize one pane (and propagate to transport).
    async fn resize(&self, pane_handle: &str, cols: u16, rows: u16) -> Result<()>;
    async fn close(&self) -> Result<()>;
}

// backends/mod.rs
#[async_trait]
pub trait PaneBackend: Send + Sync {
    async fn snapshot(&self, buf: &mut [u8]) -> Result<usize>;
    async fn write(&self, data: &[u8]) -> Result<usize>;
    async fn write_raw(&self, data: &[u8]) -> Result<usize>;
    async fn resize(&self, cols: u16, rows: u16) -> Result<()>;
    async fn close(&self) -> Result<()>;
}

// core/link.rs
pub struct Link {
    pub id: LinkId,
    pub info: LinkInfo,
    pub transport: Box<dyn Transport>,
    pub sink: Box<dyn Sink>,
    pub panes: Mutex<Vec<PaneId>>,
}
```

`SessionManager::add_link(spec, address)` returns `(LinkId, PaneId)` — the new link's id and the first pane's id (ADR 0005: always creates a brand-new link; no `--link=existing` branch). `SessionManager::open_link(spec, address)` attaches an existing tmux session (tmux only) and returns `(LinkId, PaneId)` for the registered link and its first pane.