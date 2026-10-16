# xscore CLI First Implementation Plan

**Goal:** Build the CLI-first slice of xscore — a Rust workspace that manages SSH / shell / serial sessions behind a single `Link × Pane` abstraction, callable from `xscore` CLI today and (later) from MCP server and IPC server without code rewrite.

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
- Config = `~/.config/xscore/config.json` + per-link JSON snippets under `~/.config/xscore/sessions/`.

**CLI entry:** `xscore connect <url> [--save] [--link=existing]`. URLs:

```
xscore connect shell:///bin/bash
xscore connect serial:///dev/ttyUSB0?baud=115200&data_bits=8
xscore connect ssh://user@host:22?identity=~/.ssh/id_ed25519&proxy=ssh://jump@1.2.3.4
xscore connect ssh://user@host:22?tmux=on    # default

# Multiple panes on the same link:
xscore connect ssh://u@h:22
xscore connect ssh://u@h:22 --link=existing   # attach pane 2 to the same link
xscore connect ssh://u@h:22 --link=existing   # pane 3
```

Other commands take `--id <link-id>` or `--id <pane-id>`. URLs are the entry point; `--from spec.json` is **removed**.

**Link uniqueness rule (locked from user):** A link is uniquely identified by the `(transport_kind, address-tuple)`:
- `shell` → `(command, args-tuple)`
- `serial` → `(path)`
- `ssh` → `(host, port, user, identity, proxy)`

Two `xscore connect ssh://u@h:22` calls with no `--link=existing` flag produce **different links** (each with `link_id` = a new uuid). The `--link=existing` flag opts into attach mode: search the `LinkRegistry` for a matching address tuple; if found, attach a new pane to that link; if not, error. `--link=existing` is meaningless for `shell`/`serial` and returns `Error::LinkExistingUnsupported`.

> ⚠️ **Open question (does not block the slice):** "Same link reuse" on SSH means tmux inside the SSH channel — second `xscore connect` to the same SSH with `--link=existing` should ssh-attached tmux, NOT a new SSH channel. Real russh wiring lands later; for now the slice delivers shell/serial links + ssh stubs (return error from `--link=existing` for ssh transport too, since the stub doesn't have a real channel — see acceptance criteria below).

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

**Reference:** `doc/dev/roadmap/` (this directory) — sibling to future RFCs.

---

## Module Boundaries (Three-Cut)

```
crates/xscore/
├── Cargo.toml
├── src/
│   ├── main.rs                       # clap entry, tracing init, dispatch
│   ├── cli/
│   │   ├── mod.rs                    # Cli struct, derive top-level
│   │   ├── connect.rs                # xscore connect <url> [--save] [--link=existing]
│   │   ├── link.rs                   # link list/show/close/save/load
│   │   ├── pane.rs                   # pane list/show/close
│   │   ├── data.rs                   # pane data read/write/resize
│   │   └── config.rs                 # config show/path/set/unset
│   ├── core/
│   │   ├── mod.rs                    # pub SessionManager, re-exports
│   │   ├── manager.rs                # connect/list/close for links and panes
│   │   ├── link.rs                   # Link struct (transport + sink + pane list)
│   │   ├── pane.rs                   # Pane struct (handle + state)
│   │   ├── link_registry.rs          # LinkRegistry (address-keyed dedup)
│   │   ├── pane_registry.rs          # PaneRegistry (pane_id -> PaneInfo)
│   │   ├── address.rs                # LinkAddress hashable key per transport
│   │   ├── id.rs                     # LinkId + PaneId (uuid v7)
│   │   └── error.rs                  # thiserror enum (LinkNotFound, ...)
│   ├── model/
│   │   ├── mod.rs
│   │   ├── session.rs                # ConnectSpec / LinkInfo / PaneInfo / TransportConfig / SinkConfig
│   │   └── config.rs                 # AppConfig
│   ├── url/
│   │   ├── mod.rs                    # URL parser entry
│   │   ├── scheme.rs                 # placeholder
│   │   └── connect.rs                # ConnectUrl -> ConnectSpec + LinkAddress
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

`SessionManager::connect(spec, address, mode)` returns `(LinkId, PaneId)` — the new link's id and the first pane's id. With `--link=existing`, the link is the existing match and the pane is a new pane inside it.

---

## Task List

#### Task 1: Workspace bootstrap

**Objective:** Create Rust workspace, base `xscore` crate skeleton, builds empty.

**Files:**
- Create: `Cargo.toml` (workspace)
- Create: `crates/xscore/Cargo.toml`
- Create: `crates/xscore/src/main.rs`
- Create: `crates/xscore/src/lib.rs`

**Step 1: Write workspace root**

```toml
# /home/loner/prj/xscore/Cargo.toml
[workspace]
resolver = "2"
members = ["crates/xscore"]

[workspace.package]
edition = "2021"
license = "MIT"
version = "0.1.0"

[workspace.dependencies]
tokio = { version = "1", features = ["full"] }
clap = { version = "4", features = ["derive"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
thiserror = "1"
anyhow = "1"
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter"] }
uuid = { version = "1", features = ["v7", "serde"] }
url = "2"
russh = "0.43"
russh-keys = "0.43"
portable-pty = "0.8"
tokio-serial = "0.4"
async-trait = "0.1"
dirs = "5"
mockall = "0.13"
tempfile = "3"
chrono = { version = "0.4", features = ["serde"] }
```

**Step 2: Write crate manifest**

```toml
# crates/xscore/Cargo.toml
[package]
name = "xscore"
edition.workspace = true
version.workspace = true
license.workspace = true

[dependencies]
tokio.workspace = true
clap.workspace = true
serde.workspace = true
serde_json.workspace = true
thiserror.workspace = true
anyhow.workspace = true
tracing.workspace = true
tracing-subscriber.workspace = true
uuid.workspace = true
url.workspace = true
async-trait.workspace = true
dirs.workspace = true
russh.workspace = true
russh-keys.workspace = true
portable-pty.workspace = true
tokio-serial.workspace = true

[dev-dependencies]
mockall.workspace = true
tempfile.workspace = true
```

**Step 3: Empty main.rs and lib.rs**

```rust
// crates/xscore/src/main.rs
fn main() {}
```

```rust
// crates/xscore/src/lib.rs
```

**Step 4: Build**

Run: `cargo build`
Expected: `Finished 'dev' profile`, no errors.

**Step 5: Commit**

```bash
git add Cargo.toml crates/xscore/
git commit -m "feat: scaffold xscore workspace and empty binary"
```

---

#### Task 2: Core IDs + error type + LinkAddress

**Objective:** Define `LinkId` (uuid v7), `PaneId` (uuid v7), `LinkAddress` (hashable transport identity), `core::Error`.

**Files:**
- Create: `crates/xscore/src/core/mod.rs`
- Create: `crates/xscore/src/core/id.rs`
- Create: `crates/xscore/src/core/address.rs`
- Create: `crates/xscore/src/core/error.rs`
- Create: `crates/xscore/src/core/{manager,link,pane,link_registry,pane_registry}.rs` (placeholders)

**Step 1: Write failing tests** (`id.rs` top)

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn link_id_roundtrip_via_string() {
        let id = LinkId::new();
        let s = id.to_string();
        let parsed = LinkId::parse(&s).unwrap();
        assert_eq!(id, parsed);
    }

    #[test]
    fn pane_id_roundtrip_via_string() {
        let id = PaneId::new();
        let s = id.to_string();
        let parsed = PaneId::parse(&s).unwrap();
        assert_eq!(id, parsed);
    }

    #[test]
    fn link_and_pane_ids_distinct_types() {
        // LinkId and PaneId share string form but are distinct newtypes.
        let lid = LinkId::parse("01928e0a-7d10-7000-8000-000000000000").unwrap();
        let pid = PaneId::parse("01928e0a-7d10-7000-8000-000000000001").unwrap();
        // cannot compare directly (different types); just exercise parse.
        let _ = (lid, pid);
    }
}
```

**Step 2: Implement IDs**

```rust
// core/id.rs
use serde::{Deserialize, Serialize};
use std::fmt;
use uuid::Uuid;

macro_rules! uuid_newtype {
    ($name:ident) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(Uuid);

        impl $name {
            pub fn new() -> Self { Self(Uuid::now_v7()) }
            pub fn as_uuid(&self) -> Uuid { self.0 }
            pub fn parse(s: &str) -> Result<Self, uuid::Error> {
                Ok(Self(Uuid::parse(s)?))
            }
        }

        impl Default for $name { fn default() -> Self { Self::new() } }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { self.0.fmt(f) }
        }
    };
}

uuid_newtype!(LinkId);
uuid_newtype!(PaneId);
```

**Step 3: Implement `LinkAddress`** (`core/address.rs`)

```rust
// core/address.rs
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Hash, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "transport", rename_all = "snake_case")]
pub enum LinkAddress {
    Shell { command: String, args: Vec<String> },
    Serial { path: String },
    Ssh { host: String, port: u16, user: String, identity: Option<String>, proxy: Option<String> },
}

impl LinkAddress {
    pub fn kind(&self) -> &'static str {
        match self {
            LinkAddress::Shell { .. } => "shell",
            LinkAddress::Serial { .. } => "serial",
            LinkAddress::Ssh { .. } => "ssh",
        }
    }
}
```

**Step 4: Failing test for LinkAddress**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ssh_address_equal_when_full_tuple_matches() {
        let a = LinkAddress::Ssh {
            host: "h".into(), port: 22, user: "u".into(),
            identity: Some("id".into()), proxy: None,
        };
        let b = LinkAddress::Ssh {
            host: "h".into(), port: 22, user: "u".into(),
            identity: Some("id".into()), proxy: None,
        };
        assert_eq!(a, b);
        let mut h = std::collections::HashSet::new();
        h.insert(a.clone()); h.insert(b.clone());
        assert_eq!(h.len(), 1);
    }

    #[test]
    fn ssh_address_different_when_port_differs() {
        let a = LinkAddress::Ssh {
            host: "h".into(), port: 22, user: "u".into(),
            identity: None, proxy: None,
        };
        let b = LinkAddress::Ssh {
            host: "h".into(), port: 2222, user: "u".into(),
            identity: None, proxy: None,
        };
        assert_ne!(a, b);
    }
}
```

**Step 5: Implement `core::Error`**

```rust
// core/error.rs
use thiserror::Error;
use super::id::{LinkId, PaneId};

#[derive(Debug, Error)]
pub enum Error {
    #[error("link not found: {0}")]
    LinkNotFound(LinkId),

    #[error("pane not found: {0}")]
    PaneNotFound(PaneId),

    #[error("--link=existing not supported for transport: {0}")]
    LinkExistingUnsupported(&'static str),

    #[error("url parse error: {0}")]
    Url(String),

    #[error("transport error: {0}")]
    Transport(String),

    #[error("sink error: {0}")]
    Sink(String),

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("config error: {0}")]
    Config(String),

    #[error("invalid input: {0}")]
    Invalid(String),

    #[error("serialization error: {0}")]
    Serde(#[from] serde_json::Error),
}

pub type Result<T> = std::result::Result<T, Error>;
```

**Step 6: `core/mod.rs`**

```rust
pub mod address;
pub mod error;
pub mod id;
pub mod link;
pub mod link_registry;
pub mod manager;
pub mod pane;
pub mod pane_registry;

pub use address::LinkAddress;
pub use error::{Error, Result};
pub use id::{LinkId, PaneId};
```

**Step 7: Run tests**

Run: `cargo test -p xscore core::id core::address`
Expected: 4 passed.

**Step 8: Commit**

```bash
git add crates/xscore/src/core/
git commit -m "feat(core): LinkId/PaneId (uuid v7) + LinkAddress + Error"
```

---

#### Task 3: URL → ConnectSpec + LinkAddress

**Objective:** Pure parser for `xscore connect <url>` → `(ConnectSpec, LinkAddress)`.

**Files:**
- Create: `crates/xscore/src/model/session.rs`
- Create: `crates/xscore/src/model/mod.rs`
- Create: `crates/xscore/src/url/mod.rs`
- Create: `crates/xscore/src/url/scheme.rs` (placeholder)
- Create: `crates/xscore/src/url/connect.rs`

**Step 1: Define config types** (`model/session.rs`)

```rust
// model/session.rs
use serde::{Deserialize, Serialize};
use crate::core::{LinkAddress, LinkId, PaneId};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "transport", rename_all = "snake_case")]
pub enum TransportConfig {
    Shell { command: String, #[serde(default)] args: Vec<String>, #[serde(default)] cwd: Option<String> },
    Serial { path: String, baud: u32, #[serde(default = "default_data_bits")] data_bits: u8,
             #[serde(default = "default_parity")] parity: String, #[serde(default = "default_stop_bits")] stop_bits: u8 },
    Ssh { host: String, #[serde(default = "default_ssh_port")] port: u16, user: String,
          #[serde(default)] identity: Option<String>, #[serde(default)] proxy: Option<String> },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "sink", rename_all = "snake_case")]
pub enum SinkConfig {
    Tmux {
        #[serde(default = "default_tmux_mode")]
        mode: String,
        #[serde(default)]
        session_name: Option<String>,
    },
}

fn default_ssh_port() -> u16 { 22 }
fn default_data_bits() -> u8 { 8 }
fn default_parity() -> String { "n".into() }
fn default_stop_bits() -> u8 { 1 }
fn default_tmux_mode() -> String { "auto".into() }

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ConnectSpec {
    #[serde(default = "default_name")]
    pub name: String,
    #[serde(flatten)]
    pub transport: TransportConfig,
    pub sink: SinkConfig,
}

fn default_name() -> String { uuid::Uuid::now_v7().to_string() }

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct LinkInfo {
    pub id: LinkId,
    pub name: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub address: LinkAddress,
    pub transport: TransportConfig,
    pub sink: SinkConfig,
}

impl LinkInfo {
    pub fn transport_str(&self) -> &'static str { self.address.kind() }
    pub fn sink_str(&self) -> &'static str {
        match &self.sink {
            SinkConfig::Tmux { .. } => "tmux",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PaneInfo {
    pub id: PaneId,
    pub link_id: LinkId,
    pub name: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub sink_handle: String,    // e.g. "%42"
    pub pane_index: u32,        // 0-based, set by sink
}
```

**Step 2: Write failing tests**

```rust
// url/connect.rs (test)
#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{SinkConfig, TransportConfig};
    use crate::core::LinkAddress;

    #[test]
    fn parse_ssh_url_with_tmux() {
        let (spec, addr) = parse("ssh://root@10.0.0.1:22?identity=~/.ssh/id_ed25519").unwrap();
        assert!(matches!(spec.transport, TransportConfig::Ssh { .. }));
        assert_eq!(addr, LinkAddress::Ssh {
            host: "10.0.0.1".into(), port: 22, user: "root".into(),
            identity: Some("~/.ssh/id_ed25519".into()), proxy: None,
        });
    }

    #[test]
    fn parse_shell_url_yields_shell_address() {
        let (_, addr) = parse("shell:///bin/bash").unwrap();
        assert_eq!(addr, LinkAddress::Shell { command: "/bin/bash".into(), args: vec![] });
    }

    #[test]
    fn parse_serial_url_yields_serial_address() {
        let (_, addr) = parse("serial:///dev/ttyUSB0?baud=115200").unwrap();
        assert_eq!(addr, LinkAddress::Serial { path: "/dev/ttyUSB0".into() });
    }

    #[test]
    fn parse_rejects_unknown_scheme() {
        assert!(parse("telnet://host").is_err());
    }

    #[test]
    fn ssh_with_different_identity_different_address() {
        let (_, a1) = parse("ssh://u@h:22?identity=k1").unwrap();
        let (_, a2) = parse("ssh://u@h:22?identity=k2").unwrap();
        assert_ne!(a1, a2);
    }
}
```

**Step 3: Implement parser**

```rust
// url/connect.rs
use crate::core::{Error, LinkAddress, Result};
use crate::model::{ConnectSpec, SinkConfig, TransportConfig};
use std::collections::HashMap;

pub fn parse(s: &str) -> Result<(ConnectSpec, LinkAddress)> {
    let url = url::Url::parse(s).map_err(|e| Error::Url(e.to_string()))?;
    let scheme = url.scheme();
    let host = url.host_str().map(|s| s.to_string());
    let user = (!url.username().is_empty()).then(|| url.username().to_string());

    let mut pairs: HashMap<String, String> = url.query_pairs()
        .map(|(k, v)| (k.into_owned(), v.into_owned())).collect();
    let tmux_mode = pairs.remove("tmux").unwrap_or_else(|| "auto".into());
    let name = pairs.remove("name").unwrap_or_default();

    let (transport, address) = match scheme {
        "shell" | "sh" => {
            let command = url.path().trim_start_matches('/').to_string();
            if command.is_empty() { return Err(Error::Url("shell URL needs command".into())); }
            (TransportConfig::Shell { command: command.clone(), args: vec![], cwd: None },
             LinkAddress::Shell { command, args: vec![] })
        }
        "serial" => {
            let path = url.path().to_string();
            if path.is_empty() { return Err(Error::Url("serial URL needs path".into())); }
            let baud: u32 = pairs.remove("baud")
                .ok_or_else(|| Error::Url("serial needs baud".into()))?
                .parse().map_err(|e: std::num::ParseIntError| Error::Url(e.to_string()))?;
            let data_bits: u8 = pairs.remove("data_bits").unwrap_or_else(|| "8".into()).parse()
                .map_err(|e: std::num::ParseIntError| Error::Url(e.to_string()))?;
            let parity = pairs.remove("parity").unwrap_or_else(|| "n".into());
            let stop_bits: u8 = pairs.remove("stop_bits").unwrap_or_else(|| "1".into()).parse()
                .map_err(|e: std::num::ParseIntError| Error::Url(e.to_string()))?;
            (TransportConfig::Serial { path: path.clone(), baud, data_bits, parity, stop_bits },
             LinkAddress::Serial { path })
        }
        "ssh" => {
            let host = host.ok_or_else(|| Error::Url("ssh URL needs host".into()))?;
            let user = user.ok_or_else(|| Error::Url("ssh URL needs user".into()))?;
            let port = url.port().unwrap_or(22);
            let identity = pairs.remove("identity");
            let proxy = pairs.remove("proxy");
            (TransportConfig::Ssh { host: host.clone(), port, user: user.clone(),
                                   identity: identity.clone(), proxy: proxy.clone() },
             LinkAddress::Ssh { host, port, user, identity, proxy })
        }
        other => return Err(Error::Url(format!("unknown scheme: {other}"))),
    };

    let sink = SinkConfig::Tmux {
        mode: tmux_mode,
        session_name: (!name.is_empty()).then(|| name.clone()),
    };

    Ok((ConnectSpec { name, transport, sink }, address))
}
```

```rust
// url/mod.rs
pub mod connect;
pub mod scheme;

pub use connect::parse;
```

**Step 4: Update `model/mod.rs`**

```rust
pub mod session;
pub mod config;       // placeholder

pub use session::{ConnectSpec, LinkInfo, PaneInfo, SinkConfig, TransportConfig};
```

**Step 5: Run tests**

Run: `cargo test -p xscore url`
Expected: 5 passing.

**Step 6: Commit**

```bash
git add crates/xscore/src/url/ crates/xscore/src/model/
git commit -m "feat(url): parse connect URL into (ConnectSpec, LinkAddress)"
```

---

#### Task 4: Transport trait + echo stub

**Objective:** Define `Transport` async trait with `address()` for dedup.

**Files:**
- Create: `crates/xscore/src/transport/mod.rs`
- Create: `crates/xscore/src/transport/{shell,serial,ssh}.rs` (stubs)
- Create: `crates/xscore/src/transport/factory.rs` (placeholder)

**Step 1: Failing test**

```rust
// transport/mod.rs (test)
#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;

    struct Echo;
    #[async_trait]
    impl Transport for Echo {
        fn kind(&self) -> &'static str { "echo" }
        fn address(&self) -> LinkAddress { LinkAddress::Shell { command: "echo".into(), args: vec![] } }
        async fn open(&mut self) -> Result<()> { Ok(()) }
        async fn write(&self, data: &[u8]) -> Result<usize> { Ok(data.len()) }
        async fn read(&self, _buf: &mut [u8]) -> Result<usize> { Ok(0) }
        async fn resize(&self, _: u16, _: u16) -> Result<()> { Ok(()) }
        async fn close(&self) -> Result<()> { Ok(()) }
    }

    #[tokio::test]
    async fn echo_transport_roundtrip() {
        let mut t = Echo;
        t.open().await.unwrap();
        assert_eq!(t.write(b"hi").await.unwrap(), 2);
        t.close().await.unwrap();
    }
}
```

**Step 2: Implement trait**

```rust
// transport/mod.rs
use crate::core::{LinkAddress, Result};
use async_trait::async_trait;

#[async_trait]
pub trait Transport: Send + Sync {
    fn kind(&self) -> &'static str;
    fn address(&self) -> LinkAddress;
    async fn open(&mut self) -> Result<()>;
    async fn write(&self, data: &[u8]) -> Result<usize>;
    async fn read(&self, buf: &mut [u8]) -> Result<usize>;
    async fn resize(&self, cols: u16, rows: u16) -> Result<()>;
    async fn close(&self) -> Result<()>;
}

pub mod factory;
pub mod serial;
pub mod shell;
pub mod ssh;
```

**Step 3: Run tests**

Run: `cargo test -p xscore transport`
Expected: 1 passing.

**Step 4: Commit**

```bash
git add crates/xscore/src/transport/
git commit -m "feat(transport): Transport async trait with address()"
```

---

#### Task 5: Sink trait with per-pane ops + TmuxSink + RawSink

**Objective:** Define `Sink` trait with `new_pane`/`close_pane`/`list_panes`/`snapshot(pane_handle, ...)`/`write(pane_handle, ...)`/`resize(pane_handle, ...)`. TmuxSink wraps tmux; RawSink stub.

**Files:**
- Create: `crates/xscore/src/sink/mod.rs`
- Create: `crates/xscore/src/sink/tmux.rs`
- Create: `crates/xscore/src/sink/raw.rs`
- Create: `crates/xscore/src/sink/factory.rs`
- Create: `crates/xscore/src/util/mod.rs`
- Create: `crates/xscore/src/util/tmux_socket.rs`

**Step 1: socket helper**

```rust
// util/tmux_socket.rs
pub const DEFAULT_SOCKET: &str = "xscore";
pub fn socket_name() -> String {
    std::env::var("XSCORE_SOCKET").unwrap_or_else(|_| DEFAULT_SOCKET.to_string())
}
pub fn tmux_arg_socket() -> Vec<String> { vec!["-L".into(), socket_name()] }
```

```rust
// util/mod.rs
pub mod tmux_socket;
pub use tmux_socket::{socket_name, tmux_arg_socket};
```

**Step 2: Failing test for Sink**

```rust
// sink/mod.rs (test)
#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use crate::transport::Transport;

    struct Noop;
    #[async_trait]
    impl Sink for Noop {
        fn kind(&self) -> &'static str { "noop" }
        async fn attach(&mut self, _: &dyn Transport) -> Result<()> { Ok(()) }
        async fn new_pane(&self) -> Result<String> { Ok("pane-stub".into()) }
        async fn close_pane(&self, _: &str) -> Result<()> { Ok(()) }
        async fn list_panes(&self) -> Result<Vec<String>> { Ok(vec![]) }
        async fn snapshot(&self, _: &str, _: &mut [u8]) -> Result<usize> { Ok(0) }
        async fn write(&self, _: &str, data: &[u8]) -> Result<usize> { Ok(data.len()) }
        async fn resize(&self, _: &str, _: u16, _: u16) -> Result<()> { Ok(()) }
        async fn close(&self) -> Result<()> { Ok(()) }
    }

    #[tokio::test]
    async fn noop_sink_creates_and_closes_pane() {
        let s = Noop;
        let p = s.new_pane().await.unwrap();
        assert_eq!(p, "pane-stub");
        s.close_pane(&p).await.unwrap();
    }
}
```

**Step 3: Implement Sink trait**

```rust
// sink/mod.rs
use crate::core::Result;
use crate::transport::Transport;
use async_trait::async_trait;

#[async_trait]
pub trait Sink: Send + Sync {
    fn kind(&self) -> &'static str;
    /// Wire this sink to the link's transport. Called once per link lifetime.
    async fn attach(&mut self, transport: &dyn Transport) -> Result<()>;
    /// Open a new pane inside this link. Returns sink-side handle.
    async fn new_pane(&self) -> Result<String>;
    async fn close_pane(&self, pane_handle: &str) -> Result<()>;
    async fn list_panes(&self) -> Result<Vec<String>>;
    async fn snapshot(&self, pane_handle: &str, buf: &mut [u8]) -> Result<usize>;
    async fn write(&self, pane_handle: &str, data: &[u8]) -> Result<usize>;
    async fn resize(&self, pane_handle: &str, cols: u16, rows: u16) -> Result<()>;
    async fn close(&self) -> Result<()>;
}

pub mod factory;
pub mod raw;
pub mod tmux;
```

**Step 4: TmuxSink**

```rust
// sink/tmux.rs
use crate::core::{Error, Result};
use crate::transport::Transport;
use crate::util::tmux_arg_socket;
use async_trait::async_trait;
use tokio::process::Command;

pub struct TmuxSink {
    pub mode: String,             // "local" | "remote" (post-resolution)
    pub session_name: String,     // tmux server session name == the link key
}

impl TmuxSink {
    pub async fn create(mode: &str, session_name: &str, transport: &mut dyn Transport) -> Result<Self> {
        transport.open().await?;
        let mut args = tmux_arg_socket();
        args.extend([
            "new-session".into(), "-d".into(), "-s".into(), session_name.into(),
            "-x".into(), "120".into(), "-y".into(), "30".into(),
        ]);
        let status = Command::new("tmux").args(&args).status().await?;
        if !status.success() {
            return Err(Error::Sink(format!("tmux new-session: exit {status}")));
        }
        Ok(Self { mode: mode.into(), session_name: session_name.into() })
    }

    fn tmux(&self) -> Command {
        let mut c = Command::new("tmux");
        c.args(tmux_arg_socket());
        c
    }
}

#[async_trait]
impl super::Sink for TmuxSink {
    fn kind(&self) -> &'static str { "tmux" }

    async fn attach(&mut self, _transport: &dyn Transport) -> Result<()> {
        // tmux already running on its own socket; attach is reserved for ssh-channel
        // wiring in the SSH-integration PR. No-op for now.
        Ok(())
    }

    async fn new_pane(&self) -> Result<String> {
        // tmux new-window returns pane id like "%42"; we capture stdout
        let out = self.tmux()
            .args(["new-window", "-t", &self.session_name, "-P", "-F", "#{pane_id}"])
            .output().await?;
        if !out.status.success() {
            return Err(Error::Sink(format!("tmux new-window: {:?}", out.status)));
        }
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    }

    async fn close_pane(&self, pane_handle: &str) -> Result<()> {
        let status = self.tmux()
            .args(["kill-pane", "-t", pane_handle])
            .status().await?;
        if !status.success() {
            return Err(Error::Sink(format!("kill-pane exit {status}")));
        }
        Ok(())
    }

    async fn list_panes(&self) -> Result<Vec<String>> {
        let out = self.tmux()
            .args(["list-panes", "-t", &self.session_name, "-F", "#{pane_id}"])
            .output().await?;
        if !out.status.success() {
            return Err(Error::Sink(format!("list-panes exit {:?}", out.status)));
        }
        Ok(out.lines().filter(|l| !l.is_empty()).map(String::from).collect())
    }

    async fn snapshot(&self, pane_handle: &str, buf: &mut [u8]) -> Result<usize> {
        let out = self.tmux()
            .args(["capture-pane", "-p", "-t", pane_handle, "-J"])
            .output().await?;
        if !out.status.success() {
            return Err(Error::Sink(format!("capture-pane exit {:?}", out.status)));
        }
        let bytes = &out.stdout;
        let n = bytes.len().min(buf.len());
        buf[..n].copy_from_slice(&bytes[..n]);
        Ok(n)
    }

    async fn write(&self, pane_handle: &str, data: &[u8]) -> Result<usize> {
        let s = std::str::from_utf8(data).map_err(|e| Error::Invalid(e.to_string()))?;
        let mut cmd = self.tmux();
        cmd.arg("send-keys").arg("-t").arg(pane_handle);
        for ch in s.chars() { cmd.arg(ch.to_string()); }
        let status = cmd.status().await?;
        if !status.success() { return Err(Error::Sink(format!("send-keys exit {status}"))); }
        Ok(data.len())
    }

    async fn resize(&self, pane_handle: &str, cols: u16, rows: u16) -> Result<()> {
        let status = self.tmux()
            .args(["resize-pane", "-t", pane_handle,
                   "-x", &cols.to_string(), "-y", &rows.to_string()])
            .status().await?;
        if !status.success() { return Err(Error::Sink(format!("resize-pane exit {status}"))); }
        Ok(())
    }

    async fn close(&self) -> Result<()> {
        let _ = self.tmux()
            .args(["kill-session", "-t", &self.session_name])
            .status().await?;
        Ok(())
    }
}
```

**Step 5: RawSink** (one pane per link; no-op semantics)

```rust
// sink/raw.rs
use crate::core::Result;
use crate::transport::Transport;
use async_trait::async_trait;

pub struct RawSink;

#[async_trait]
impl super::Sink for RawSink {
    fn kind(&self) -> &'static str { "raw" }
    async fn attach(&mut self, _: &dyn Transport) -> Result<()> { Ok(()) }
    async fn new_pane(&self) -> Result<String> { Ok("raw-pane-0".into()) }
    async fn close_pane(&self, _: &str) -> Result<()> { Ok(()) }
    async fn list_panes(&self) -> Result<Vec<String>> { Ok(vec!["raw-pane-0".into()]) }
    async fn snapshot(&self, _: &str, _: &mut [u8]) -> Result<usize> { Ok(0) }
    async fn write(&self, _: &str, _: &[u8]) -> Result<usize> { Ok(0) }
    async fn resize(&self, _: &str, _: u16, _: u16) -> Result<()> { Ok(()) }
    async fn close(&self) -> Result<()> { Ok(()) }
}
```

**Step 6: factory**

```rust
// sink/factory.rs
use crate::core::Result;
use crate::model::SinkConfig;
use crate::transport::Transport;
use super::{raw::RawSink, tmux::TmuxSink, Sink};

pub async fn sink_for(
    config: &SinkConfig,
    transport_kind: &str,
    transport: &mut dyn Transport,
) -> Result<Box<dyn Sink>> {
    match config {
        SinkConfig::Tmux { mode, session_name } => {
            let resolved_mode = match mode.as_str() {
                "auto" => if transport_kind == "ssh" { "remote" } else { "local" },
                other => other,
            };
            if resolved_mode == "off" {
                return Ok(Box::new(RawSink));
            }
            let name = session_name.clone()
                .unwrap_or_else(|| format!("xscore-{}", uuid::Uuid::now_v7()));
            let s = TmuxSink::create(resolved_mode, &name, transport).await?;
            Ok(Box::new(s))
        }
    }
}
```

**Step 7: Run tests**

Run: `cargo test -p xscore sink:: util::`
Expected: passing.

**Step 8: Commit**

```bash
git add crates/xscore/src/sink/ crates/xscore/src/util/
git commit -m "feat(sink): Sink trait with per-pane ops + TmuxSink (multi-pane) + RawSink"
```

---

#### Task 6: Link + Pane + Registries + SessionManager

**Objective:** LinkRegistry (address-keyed dedup), PaneRegistry, BackendRegistry, SessionManager that knows about all three.

**Files:**
- Create: `crates/xscore/src/core/link.rs`
- Create: `crates/xscore/src/core/pane.rs`
- Create: `crates/xscore/src/core/link_registry.rs`
- Create: `crates/xscore/src/core/pane_registry.rs`
- Create: `crates/xscore/src/backends/mod.rs`
- Create: `crates/xscore/src/backends/composite.rs`
- Create: `crates/xscore/src/transport/factory.rs` (real impl)
- Modify: `crates/xscore/src/core/manager.rs`

**Step 1: Link** (`core/link.rs`)

```rust
// core/link.rs
use std::sync::Arc;
use tokio::sync::Mutex;
use super::id::{LinkId, PaneId};
use crate::model::LinkInfo;
use crate::sink::Sink;
use crate::transport::Transport;

pub struct Link {
    pub id: LinkId,
    pub info: LinkInfo,
    pub transport: Box<dyn Transport>,
    pub sink: Box<dyn Sink>,
    pub panes: Mutex<Vec<PaneId>>,
}

pub type ArcLink = Arc<Link>;
```

**Step 2: Pane** (`core/pane.rs`)

```rust
// core/pane.rs
use super::id::{LinkId, PaneId};
use crate::model::PaneInfo;

pub struct Pane {
    pub info: PaneInfo,
}

impl Pane {
    pub fn id(&self) -> PaneId { self.info.id }
    pub fn link_id(&self) -> LinkId { self.info.link_id }
}
```

**Step 3: LinkRegistry** (`core/link_registry.rs`)

```rust
// core/link_registry.rs
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use super::address::LinkAddress;
use super::id::LinkId;
use super::link::{ArcLink, Link};

#[derive(Default)]
pub struct LinkRegistry {
    inner: RwLock<LinkRegistryInner>,
}

#[derive(Default)]
struct LinkRegistryInner {
    by_id: HashMap<LinkId, ArcLink>,
    by_address: HashMap<LinkAddress, LinkId>,
}

impl LinkRegistry {
    pub fn new() -> Self { Self::default() }

    pub async fn insert(&self, link: Link) -> ArcLink {
        let id = link.id;
        let address = link.info.address.clone();
        let arc = Arc::new(link);
        let mut g = self.inner.write().await;
        g.by_address.insert(address, id);
        g.by_id.insert(id, arc.clone());
        arc
    }

    pub async fn get(&self, id: LinkId) -> Option<ArcLink> {
        self.inner.read().await.by_id.get(&id).cloned()
    }

    pub async fn find_by_address(&self, address: &LinkAddress) -> Option<ArcLink> {
        let g = self.inner.read().await;
        let id = g.by_address.get(address).copied()?;
        g.by_id.get(&id).cloned()
    }

    pub async fn list(&self) -> Vec<ArcLink> {
        self.inner.read().await.by_id.values().cloned().collect()
    }

    pub async fn remove(&self, id: LinkId) -> Option<ArcLink> {
        let mut g = self.inner.write().await;
        let arc = g.by_id.remove(&id)?;
        let info = &arc.info;
        let addr = info.address.clone();
        // only delete from by_address if still pointing at this id (id may have been re-inserted)
        if g.by_address.get(&addr) == Some(&id) {
            g.by_address.remove(&addr);
        }
        Some(arc)
    }
}
```

**Step 4: PaneRegistry** (`core/pane_registry.rs`)

```rust
// core/pane_registry.rs
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use super::id::{LinkId, PaneId};
use crate::model::PaneInfo;

#[derive(Default)]
pub struct PaneRegistry {
    inner: RwLock<HashMap<PaneId, PaneInfo>>,
}

impl PaneRegistry {
    pub fn new() -> Self { Self::default() }
    pub async fn insert(&self, info: PaneInfo) { self.inner.write().await.insert(info.id, info); }
    pub async fn get(&self, id: PaneId) -> Option<PaneInfo> { self.inner.read().await.get(&id).cloned() }
    pub async fn remove(&self, id: PaneId) -> Option<PaneInfo> { self.inner.write().await.remove(&id) }
    pub async fn list_for_link(&self, link_id: LinkId) -> Vec<PaneInfo> {
        self.inner.read().await.values().filter(|p| p.link_id == link_id).cloned().collect()
    }
    pub async fn list_all(&self) -> Vec<PaneInfo> {
        self.inner.read().await.values().cloned().collect()
    }
}

pub type ArcPaneRegistry = Arc<PaneRegistry>;
```

**Step 5: Transport factory** (`transport/factory.rs`)

```rust
// transport/factory.rs
use crate::core::Result;
use crate::model::TransportConfig;
use super::{serial::SerialTransport, shell::ShellTransport, ssh::SshTransport, Transport};

pub async fn transport_for(config: TransportConfig) -> Result<Box<dyn Transport>> {
    match config {
        TransportConfig::Shell { command, args, cwd } => {
            Ok(Box::new(ShellTransport::new(command, args, cwd).await?))
        }
        TransportConfig::Serial { path, baud, data_bits, parity, stop_bits } => {
            Ok(Box::new(SerialTransport::new(path, baud, data_bits, parity, stop_bits).await?))
        }
        TransportConfig::Ssh { host, port, user, identity, proxy } => {
            Ok(Box::new(SshTransport::connect(host, port, user, identity, proxy).await?))
        }
    }
}
```

**Step 6: PaneBackend + Composite** (`backends/`)

```rust
// backends/mod.rs
use crate::core::Result;
use crate::core::PaneId;
use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

#[async_trait]
pub trait PaneBackend: Send + Sync {
    async fn snapshot(&self, buf: &mut [u8]) -> Result<usize>;
    async fn write(&self, data: &[u8]) -> Result<usize>;
    async fn write_raw(&self, data: &[u8]) -> Result<usize>;
    async fn resize(&self, cols: u16, rows: u16) -> Result<()>;
    async fn close(&self) -> Result<()>;
}

#[derive(Default)]
pub struct BackendRegistry {
    inner: RwLock<HashMap<PaneId, Arc<dyn PaneBackend>>>,
}

impl BackendRegistry {
    pub fn new() -> Self { Self::default() }
    pub async fn insert(&self, id: PaneId, b: Arc<dyn PaneBackend>) { self.inner.write().await.insert(id, b); }
    pub async fn get(&self, id: PaneId) -> Option<Arc<dyn PaneBackend>> { self.inner.read().await.get(&id).cloned() }
    pub async fn remove(&self, id: PaneId) -> Option<Arc<dyn PaneBackend>> { self.inner.write().await.remove(&id) }
}

pub mod composite;
```

```rust
// backends/composite.rs
use crate::core::link::ArcLink;
use crate::core::PaneId;
use crate::core::Result;
use async_trait::async_trait;

pub struct CompositeBackend {
    pane_id: PaneId,
    link: ArcLink,
    sink_handle: String,
}

impl CompositeBackend {
    pub fn new(pane_id: PaneId, link: ArcLink, sink_handle: String) -> Self {
        Self { pane_id, link, sink_handle }
    }
}

#[async_trait]
impl super::PaneBackend for CompositeBackend {
    async fn snapshot(&self, buf: &mut [u8]) -> Result<usize> {
        self.link.sink.snapshot(&self.sink_handle, buf).await
    }
    async fn write(&self, data: &[u8]) -> Result<usize> {
        self.link.sink.write(&self.sink_handle, data).await
    }
    async fn write_raw(&self, data: &[u8]) -> Result<usize> {
        self.link.transport.write(data).await
    }
    async fn resize(&self, cols: u16, rows: u16) -> Result<()> {
        self.link.transport.resize(cols, rows).await?;
        self.link.sink.resize(&self.sink_handle, cols, rows).await?;
        Ok(())
    }
    async fn close(&self) -> Result<()> {
        // close the pane only — link survives
        self.link.sink.close_pane(&self.sink_handle).await
    }
}
```

**Step 7: SessionManager** (`core/manager.rs`)

```rust
// core/manager.rs
use std::sync::Arc;
use super::{Error, LinkAddress};
use super::id::{LinkId, PaneId};
use super::link::Link;
use super::link_registry::LinkRegistry;
use super::pane_registry::PaneRegistry;
use crate::backends::{BackendRegistry, PaneBackend};
use crate::backends::composite::CompositeBackend;
use crate::model::{LinkInfo, PaneInfo};
use crate::sink::factory::sink_for;
use crate::transport::factory as transport_factory;
use crate::model::ConnectSpec;

#[derive(Clone)]
pub struct SessionManager {
    pub links: Arc<LinkRegistry>,
    pub panes: Arc<PaneRegistry>,
    pub backends: Arc<BackendRegistry>,
}

impl Default for SessionManager {
    fn default() -> Self { Self::new() }
}

impl SessionManager {
    pub fn new() -> Self {
        Self {
            links: Arc::new(LinkRegistry::new()),
            panes: Arc::new(PaneRegistry::new()),
            backends: Arc::new(BackendRegistry::new()),
        }
    }

    /// Connect: returns (link_id, pane_id).
    /// - If `link_existing=false`: always create a new link (with new address uuid).
    /// - If `link_existing=true`: search by LinkAddress; if found, attach new pane;
    ///   if not found, return Error::LinkExistingUnsupported (for non-ssh) /
    ///   Error::LinkNotFound for ssh-but-not-yet-implemented (deferred to SSH phase).
    pub async fn connect(
        &self,
        spec: ConnectSpec,
        address: LinkAddress,
        link_existing: bool,
    ) -> Result<(LinkId, PaneId)> {
        if link_existing {
            // For ssh: defer until real russh wiring (returns NotFound since stub never opens).
            // For shell/serial: refuse up front — no "same link" semantics.
            match address.kind() {
                "shell" | "serial" => return Err(Error::LinkExistingUnsupported(address.kind())),
                "ssh" => {
                    // Search first; if found, attach. If not, error (real russh channel
                    // opening for "ssh --link=existing" lands in SSH-integration PR).
                    if self.links.find_by_address(&address).await.is_none() {
                        return Err(Error::LinkNotFound(LinkId::default()));
                    }
                }
                _ => unreachable!(),
            }
        }

        // Resolve the link to use.
        let arc_link = match (link_existing, self.links.find_by_address(&address).await) {
            (true, Some(existing)) => existing,
            _ => {
                // Create a brand new link.
                let mut transport = transport_factory::transport_for(spec.transport.clone()).await?;
                let transport_kind = transport.kind();
                let mut sink = sink_for(&spec.sink, transport_kind, transport.as_mut()).await?;
                let mut transport = transport; // Box<dyn Transport>
                sink.attach(transport.as_mut()).await?;

                let link_id = LinkId::new();
                let link_info = LinkInfo {
                    id: link_id,
                    name: if spec.name.is_empty() { link_id.to_string() } else { spec.name.clone() },
                    created_at: chrono::Utc::now(),
                    address: address.clone(),
                    transport: spec.transport.clone(),
                    sink: spec.sink.clone(),
                };
                let link = Link {
                    id: link_id,
                    info: link_info,
                    transport,
                    sink,
                    panes: tokio::sync::Mutex::new(Vec::new()),
                };
                self.links.insert(link).await
            }
        };

        // Open a new pane inside the link.
        let sink_handle = arc_link.sink.new_pane().await?;
        let pane_index = arc_link.panes.lock().await.len() as u32;
        let pane_id = PaneId::new();
        let pane_name = format!("pane-{}", pane_index);
        let pane_info = PaneInfo {
            id: pane_id,
            link_id: arc_link.id,
            name: pane_name,
            created_at: chrono::Utc::now(),
            sink_handle: sink_handle.clone(),
            pane_index,
        };

        arc_link.panes.lock().await.push(pane_id);
        self.panes.insert(pane_info).await;

        let backend: Arc<dyn PaneBackend> = Arc::new(CompositeBackend::new(pane_id, arc_link.clone(), sink_handle));
        self.backends.insert(pane_id, backend).await;

        Ok((arc_link.id, pane_id))
    }

    pub async fn backend(&self, id: PaneId) -> Result<Arc<dyn PaneBackend>> {
        self.backends.get(id).await.ok_or(Error::PaneNotFound(id))
    }

    pub async fn close_link(&self, id: LinkId) -> Result<()> {
        let arc_link = self.links.get(id).await.ok_or(Error::LinkNotFound(id))?;
        // remove all panes
        let panes = arc_link.panes.lock().await.clone();
        for pid in panes {
            self.backends.remove(pid).await;
            self.panes.remove(pid).await;
        }
        // close sink + transport
        arc_link.sink.close().await?;
        arc_link.transport.close().await?;
        // remove from registry (ArcLink stays alive via this stack frame until end of fn)
        self.links.remove(id).await;
        Ok(())
    }

    pub async fn close_pane(&self, id: PaneId) -> Result<()> {
        let backend = self.backends.remove(id).await.ok_or(Error::PaneNotFound(id))?;
        let info = self.panes.remove(id).await.ok_or(Error::PaneNotFound(id))?;
        backend.close().await?;
        if let Some(link) = self.links.get(info.link_id).await {
            link.panes.lock().await.retain(|p| *p != id);
        }
        Ok(())
    }
}
```

**Step 8: Run tests**

Run: `cargo test -p xscore`
Expected: existing tests still pass.

**Step 9: Commit**

```bash
git add crates/xscore/src/core/ crates/xscore/src/backends/ crates/xscore/src/transport/factory.rs crates/xscore/src/sink/
git commit -m "feat(core): Link + Pane + Registries + SessionManager.connect (with --link=existing)"
```

---

#### Task 7: ShellTransport (real)

**Objective:** `Transport` impl that spawns a local child with stdin/stdout pipes; `address()` returns `LinkAddress::Shell`.

**Files:**
- Modify: `crates/xscore/src/transport/shell.rs`

```rust
// transport/shell.rs
use crate::core::{Error, LinkAddress, Result};
use async_trait::async_trait;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::{Child, Command};
use tokio::sync::Mutex;

pub struct ShellTransport {
    pub command: String,
    pub args: Vec<String>,
    pub cwd: Option<String>,
    child: Mutex<Option<Child>>,
}

impl ShellTransport {
    pub async fn new(command: String, args: Vec<String>, cwd: Option<String>) -> Result<Self> {
        Ok(Self { command, args, cwd, child: Mutex::new(None) })
    }
}

#[async_trait]
impl super::Transport for ShellTransport {
    fn kind(&self) -> &'static str { "shell" }
    fn address(&self) -> LinkAddress {
        LinkAddress::Shell { command: self.command.clone(), args: self.args.clone() }
    }

    async fn open(&mut self) -> Result<()> {
        let mut cmd = Command::new(&self.command);
        cmd.args(&self.args);
        if let Some(c) = &self.cwd { cmd.current_dir(c); }
        cmd.stdin(std::process::Stdio::piped()).stdout(std::process::Stdio::piped());
        let child = cmd.spawn().map_err(|e| Error::Transport(format!("spawn: {e}")))?;
        *self.child.lock().await = Some(child);
        Ok(())
    }

    async fn write(&self, data: &[u8]) -> Result<usize> {
        let mut g = self.child.lock().await;
        let c = g.as_mut().ok_or_else(|| Error::Transport("not open".into()))?;
        let stdin = c.stdin.as_mut().ok_or_else(|| Error::Transport("stdin closed".into()))?;
        stdin.write_all(data).await?;
        Ok(data.len())
    }

    async fn read(&self, buf: &mut [u8]) -> Result<usize> {
        let mut g = self.child.lock().await;
        let c = g.as_mut().ok_or_else(|| Error::Transport("not open".into()))?;
        let stdout = c.stdout.as_mut().ok_or_else(|| Error::Transport("stdout closed".into()))?;
        Ok(stdout.read(buf).await?)
    }

    async fn resize(&self, _: u16, _: u16) -> Result<()> { Ok(()) }
    async fn close(&self) -> Result<()> {
        let mut g = self.child.lock().await;
        if let Some(c) = g.as_mut() { c.kill().await?; }
        *g = None;
        Ok(())
    }
}
```

**Step 1: Test**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn shell_transport_roundtrip() {
        let mut t = ShellTransport::new("cat".into(), vec![], None).await.unwrap();
        t.open().await.unwrap();
        t.write(b"hi\n").await.unwrap();
        let mut buf = [0u8; 16];
        let n = t.read(&mut buf).await.unwrap();
        assert!(std::str::from_utf8(&buf[..n]).unwrap().contains("hi"));
        t.close().await.unwrap();
    }
}
```

**Step 2: Commit**

```bash
git add crates/xscore/src/transport/shell.rs
git commit -m "feat(transport/shell): real ShellTransport with address()"
```

---

#### Task 8: SerialTransport (stub) + SshTransport (stub)

Same shape as Task 7 but with `address()` returning the corresponding `LinkAddress` variant.

**Files:**
- Modify: `crates/xscore/src/transport/serial.rs`
- Modify: `crates/xscore/src/transport/ssh.rs`

```rust
// transport/serial.rs
use crate::core::{Error, LinkAddress, Result};
use async_trait::async_trait;

pub struct SerialTransport { pub path: String, pub baud: u32, pub data_bits: u8, pub parity: String, pub stop_bits: u8 }

impl SerialTransport {
    pub async fn new(path: String, baud: u32, data_bits: u8, parity: String, stop_bits: u8) -> Result<Self> {
        Ok(Self { path, baud, data_bits, parity, stop_bits })
    }
}

#[async_trait]
impl super::Transport for SerialTransport {
    fn kind(&self) -> &'static str { "serial" }
    fn address(&self) -> LinkAddress { LinkAddress::Serial { path: self.path.clone() } }
    async fn open(&mut self) -> Result<()> { Err(Error::Transport("SerialTransport::open pending integration phase".into())) }
    async fn write(&self, _: &[u8]) -> Result<usize> { Err(Error::Transport("not open".into())) }
    async fn read(&self, _: &mut [u8]) -> Result<usize> { Err(Error::Transport("not open".into())) }
    async fn resize(&self, _: u16, _: u16) -> Result<()> { Ok(()) }
    async fn close(&self) -> Result<()> { Ok(()) }
}
```

```rust
// transport/ssh.rs
use crate::core::{Error, LinkAddress, Result};
use async_trait::async_trait;

pub struct SshTransport {
    pub host: String, pub port: u16, pub user: String,
    pub identity: Option<String>, pub proxy: Option<String>,
}

impl SshTransport {
    pub async fn connect(host: String, port: u16, user: String, identity: Option<String>, proxy: Option<String>) -> Result<Self> {
        Ok(Self { host, port, user, identity, proxy })
    }
}

#[async_trait]
impl super::Transport for SshTransport {
    fn kind(&self) -> &'static str { "ssh" }
    fn address(&self) -> LinkAddress {
        LinkAddress::Ssh {
            host: self.host.clone(), port: self.port, user: self.user.clone(),
            identity: self.identity.clone(), proxy: self.proxy.clone(),
        }
    }
    async fn open(&mut self) -> Result<()> { Err(Error::Transport("SshTransport::open pending integration phase".into())) }
    async fn write(&self, _: &[u8]) -> Result<usize> { Err(Error::Transport("not open".into())) }
    async fn read(&self, _: &mut [u8]) -> Result<usize> { Err(Error::Transport("not open".into())) }
    async fn resize(&self, _: u16, _: u16) -> Result<()> { Ok(()) }
    async fn close(&self) -> Result<()> { Ok(()) }
}
```

**Step 1: Compile + commit**

```bash
cargo build -p xscore
git add crates/xscore/src/transport/serial.rs crates/xscore/src/transport/ssh.rs
git commit -m "feat(transport): Serial/Ssh stubs with address()"
```

---

#### Task 9: Config loader

Save/load `ConnectSpec` JSON to `~/.config/xscore/sessions/link-<link-id>.json`.

**Files:**
- Create: `crates/xscore/src/config/{mod,paths,loader}.rs`

```rust
// config/paths.rs
pub const APP_DIR: &str = "xscore";
pub fn config_dir() -> std::path::PathBuf {
    dirs::config_dir().unwrap_or_else(|| std::path::PathBuf::from(".config")).join(APP_DIR)
}
pub fn sessions_dir() -> std::path::PathBuf { config_dir().join("sessions") }
pub fn config_file() -> std::path::PathBuf { config_dir().join("config.json") }
pub fn ensure_dirs() -> std::io::Result<()> {
    std::fs::create_dir_all(config_dir())?;
    std::fs::create_dir_all(sessions_dir())?;
    Ok(())
}
pub fn link_path(link_id: &str) -> std::path::PathBuf { sessions_dir().join(format!("link-{link_id}.json")) }
```

```rust
// config/loader.rs
use crate::core::Result;
use crate::model::ConnectSpec;
use std::path::Path;

pub fn save_connect_spec(path: &Path, spec: &ConnectSpec) -> Result<()> {
    if let Some(parent) = path.parent() { std::fs::create_dir_all(parent)?; }
    std::fs::write(path, serde_json::to_string_pretty(spec)?)?;
    Ok(())
}
pub fn load_connect_spec(path: &Path) -> Result<ConnectSpec> {
    Ok(serde_json::from_str(&std::fs::read_to_string(path)?)?)
}
```

```rust
// config/mod.rs
pub mod loader;
pub mod paths;

pub use loader::{load_connect_spec, save_connect_spec};
pub use paths::{config_dir, config_file, ensure_dirs, link_path, sessions_dir};
```

**Step 1: Commit**

```bash
git add crates/xscore/src/config/
git commit -m "feat(config): XDG paths + ConnectSpec save/load"
```

---

#### Task 10: CLI — `connect`, `link`, `pane` subcommands

**Objective:** Wire `connect <url> [--save] [--link=existing]`, `link list/show/close/save/load`, `pane list/show/close`.

**Files:**
- Create: `crates/xscore/src/cli/{mod,connect,link,pane}.rs`
- Modify: `crates/xscore/src/main.rs`

**Step 1: cli/mod.rs**

```rust
// cli/mod.rs
pub mod config;
pub mod connect;
pub mod data;
pub mod link;
pub mod pane;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "xscore", version, about = "Manage SSH / shell / serial sessions as Link x Pane")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Subcommand)]
pub enum Commands {
    /// Open a session from a URL (creates or attaches to a link, returns a pane id)
    Connect(connect::ConnectArgs),
    /// Inspect / close a link (the physical connection)
    Link(link::LinkCmd),
    /// Inspect / close a pane (one view into a link)
    Pane(pane::PaneCmd),
    /// Read / write / resize data on a pane
    Data(data::DataCmd),
    /// Show / edit config values
    Config(config::ConfigCmd),
}
```

**Step 2: cli/connect.rs**

```rust
// cli/connect.rs
use clap::Args;
use std::sync::Arc;

#[derive(Args)]
pub struct ConnectArgs {
    /// URL: shell://, ssh://, serial://
    pub url: String,

    /// Save the parsed spec to ~/.config/xscore/sessions/link-<id>.json
    #[arg(long)]
    pub save: bool,

    /// Attach a new pane to an existing link matching the address tuple.
    #[arg(long)]
    pub link_existing: bool,
}

pub async fn run(mgr: Arc<SessionManager>, args: ConnectArgs) -> anyhow::Result<()> {
    let (spec, address) = xscore::url::parse(&args.url)?;
    let (link_id, pane_id) = mgr.connect(spec.clone(), address, args.link_existing).await?;
    eprintln!("link:  {link_id}");
    println!("pane:  {pane_id}");
    if args.save {
        xscore::config::ensure_dirs()?;
        xscore::config::save_connect_spec(
            &xscore::config::link_path(&link_id.to_string()), &spec)?;
    }
    Ok(())
}
```

**Step 3: cli/link.rs**

```rust
// cli/link.rs
use crate::core::LinkId;
use crate::core::SessionManager;
use clap::Subcommand;
use std::sync::Arc;

#[derive(Subcommand)]
pub enum LinkCmd {
    List,
    Show { #[arg(long)] id: String },
    Close { #[arg(long)] id: String },
    Save { #[arg(long)] id: String },
    Load { #[arg(long)] path: String },
}

pub async fn run(mgr: Arc<SessionManager>, cmd: LinkCmd) -> anyhow::Result<()> {
    match cmd {
        LinkCmd::List => {
            for link in mgr.links.list().await {
                let info = &link.info;
                println!("{}\t{}\t{}", info.id, info.transport_str(), info.name);
            }
        }
        LinkCmd::Show { id } => {
            let lid = LinkId::parse(&id)?;
            let link = mgr.links.get(lid).await.ok_or(crate::core::Error::LinkNotFound(lid))?;
            println!("{}", serde_json::to_string_pretty(&link.info)?);
        }
        LinkCmd::Close { id } => {
            let lid = LinkId::parse(&id)?;
            mgr.close_link(lid).await?;
            println!("closed link: {lid}");
        }
        LinkCmd::Save { id } => {
            let lid = LinkId::parse(&id)?;
            let link = mgr.links.get(lid).await.ok_or(crate::core::Error::LinkNotFound(lid))?;
            let spec = link_to_spec(&link.info);
            xscore::config::ensure_dirs()?;
            xscore::config::save_connect_spec(
                &xscore::config::link_path(&lid.to_string()), &spec)?;
            println!("saved: {lid}");
        }
        LinkCmd::Load { path } => {
            let spec = xscore::config::load_connect_spec(std::path::Path::new(&path))?;
            println!("{}", serde_json::to_string_pretty(&spec)?);
        }
    }
    Ok(())
}

fn link_to_spec(info: &crate::model::LinkInfo) -> xscore::model::ConnectSpec {
    xscore::model::ConnectSpec {
        name: info.name.clone(),
        transport: info.transport.clone(),
        sink: info.sink.clone(),
    }
}
```

**Step 4: cli/pane.rs**

```rust
// cli/pane.rs
use crate::core::PaneId;
use crate::core::SessionManager;
use clap::Subcommand;
use std::sync::Arc;

#[derive(Subcommand)]
pub enum PaneCmd {
    /// List panes (optionally filtered by --link <link-id>)
    List { #[arg(long)] link: Option<String> },
    Show { #[arg(long)] id: String },
    Close { #[arg(long)] id: String },
}

pub async fn run(mgr: Arc<SessionManager>, cmd: PaneCmd) -> anyhow::Result<()> {
    match cmd {
        PaneCmd::List { link } => {
            let all = mgr.panes.list_all().await;
            for p in all {
                if let Some(lid) = &link {
                    if p.link_id.to_string() != *lid { continue; }
                }
                println!("{}\t{}\t{}", p.id, p.link_id, p.sink_handle);
            }
        }
        PaneCmd::Show { id } => {
            let pid = PaneId::parse(&id)?;
            let info = mgr.panes.get(pid).await.ok_or(crate::core::Error::PaneNotFound(pid))?;
            println!("{}", serde_json::to_string_pretty(&info)?);
        }
        PaneCmd::Close { id } => {
            let pid = PaneId::parse(&id)?;
            mgr.close_pane(pid).await?;
            println!("closed pane: {pid}");
        }
    }
    Ok(())
}
```

**Step 5: main.rs**

```rust
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();
    let cli = xscore::cli::Cli::parse();
    let mgr = std::sync::Arc::new(xscore::core::SessionManager::new());
    match cli.command {
        xscore::cli::Commands::Connect(c) => xscore::cli::connect::run(mgr, c).await?,
        xscore::cli::Commands::Link(c) => xscore::cli::link::run(mgr, c).await?,
        xscore::cli::Commands::Pane(c) => xscore::cli::pane::run(mgr, c).await?,
        xscore::cli::Commands::Data(_c) => println!("data (todo)"),
        xscore::cli::Commands::Config(_c) => println!("config (todo)"),
    }
    Ok(())
}
```

**Step 6: Smoke**

```bash
cargo run -p xscore -- connect 'shell:///bin/cat' --save
cargo run -p xscore -- link list
cargo run -p xscore -- pane list
cargo run -p xscore -- pane close --id <pane-id>     # closes pane, link stays
cargo run -p xscore -- link close --id <link-id>     # closes the connection
```

**Step 7: Commit**

```bash
git add crates/xscore/src/cli/ crates/xscore/src/main.rs
git commit -m "feat(cli): connect/link/pane subcommands over Link x Pane"
```

---

#### Task 11: CLI `data` subcommand — read/write/resize

Per-pane data ops (the only place pane_id is needed).

**Files:**
- Modify: `crates/xscore/src/cli/data.rs`

```rust
// cli/data.rs
use crate::core::PaneId;
use crate::core::SessionManager;
use clap::Subcommand;
use std::sync::Arc;

#[derive(Subcommand)]
pub enum DataCmd {
    Read { #[arg(long)] pane: String, #[arg(long, default_value_t = 4096)] bytes: usize },
    Write { #[arg(long)] pane: String, #[arg(long)] from_file: Option<String> },
    Resize { #[arg(long)] pane: String, #[arg(long)] cols: u16, #[arg(long)] rows: u16 },
}

pub async fn run(mgr: Arc<SessionManager>, cmd: DataCmd) -> anyhow::Result<()> {
    match cmd {
        DataCmd::Read { pane, bytes } => {
            let pid = PaneId::parse(&pane)?;
            let backend = mgr.backend(pid).await?;
            let mut buf = vec![0u8; bytes];
            let n = backend.snapshot(&mut buf).await?;
            std::io::Write::write_all(&mut std::io::stdout(), &buf[..n])?;
        }
        DataCmd::Write { pane, from_file } => {
            let pid = PaneId::parse(&pane)?;
            let backend = mgr.backend(pid).await?;
            let data = if let Some(p) = from_file { std::fs::read(p)? } else {
                let mut s = String::new();
                std::io::Read::read_to_string(&mut std::io::stdin(), &mut s)?;
                s.into_bytes()
            };
            let n = backend.write(&data).await?;
            eprintln!("wrote {n} bytes");
        }
        DataCmd::Resize { pane, cols, rows } => {
            let pid = PaneId::parse(&pane)?;
            let backend = mgr.backend(pid).await?;
            backend.resize(cols, rows).await?;
            eprintln!("resized to {cols}x{rows}");
        }
    }
    Ok(())
}
```

**Step 2: Wire to main**

```rust
match cli.command {
    // ...
    xscore::cli::Commands::Data(c) => xscore::cli::data::run(mgr, c).await?,
    // ...
}
```

**Step 3: Commit**

```bash
git add crates/xscore/src/cli/data.rs crates/xscore/src/main.rs
git commit -m "feat(cli): data read/write/resize over pane-id"
```

---

#### Task 12: CLI `config` subcommand

Same shape as before (AppConfig JSON).

**Files:**
- Modify: `crates/xscore/src/cli/config.rs`
- Modify: `crates/xscore/src/model/config.rs`

```rust
// model/config.rs
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    #[serde(default)]
    pub defaults: HashMap<String, serde_json::Value>,
    #[serde(default)]
    pub tmux_socket: Option<String>,
}
```

```rust
// cli/config.rs
use crate::config;
use crate::model::AppConfig;
use clap::Subcommand;

#[derive(Subcommand)]
pub enum ConfigCmd {
    Show,
    Path,
    Set { key: String, value: String },
    Unset { key: String },
}

pub async fn run(cmd: ConfigCmd) -> anyhow::Result<()> {
    config::ensure_dirs()?;
    match cmd {
        ConfigCmd::Show => {
            let cfg: AppConfig = if config::config_file().exists() {
                serde_json::from_str(&std::fs::read_to_string(config::config_file())?)?
            } else { AppConfig::default() };
            println!("{}", serde_json::to_string_pretty(&cfg)?);
        }
        ConfigCmd::Path => {
            println!("config: {}", config::config_file().display());
            println!("sessions: {}", config::sessions_dir().display());
        }
        ConfigCmd::Set { key, value } => {
            let mut cfg = load_or_default();
            apply_set(&mut cfg, &key, &value);
            save(&cfg)?;
            println!("set {key} = {value}");
        }
        ConfigCmd::Unset { key } => {
            let mut cfg = load_or_default();
            apply_unset(&mut cfg, &key);
            save(&cfg)?;
            println!("unset {key}");
        }
    }
    Ok(())
}

fn load_or_default() -> AppConfig {
    if config::config_file().exists() {
        serde_json::from_str(&std::fs::read_to_string(config::config_file()).unwrap_or_default())
            .unwrap_or_default()
    } else { AppConfig::default() }
}

fn save(cfg: &AppConfig) -> anyhow::Result<()> {
    config::ensure_dirs()?;
    std::fs::write(config::config_file(), serde_json::to_string_pretty(cfg)?)?;
    Ok(())
}

fn apply_set(cfg: &mut AppConfig, key: &str, value: &str) {
    match key {
        "tmux_socket" => cfg.tmux_socket = Some(value.into()),
        _ => { cfg.defaults.insert(key.into(), serde_json::Value::String(value.into())); }
    }
}

fn apply_unset(cfg: &mut AppConfig, key: &str) {
    if key == "tmux_socket" { cfg.tmux_socket = None; }
    else { cfg.defaults.remove(key); }
}
```

**Step 4: Commit**

```bash
git add crates/xscore/src/cli/config.rs crates/xscore/src/model/config.rs
git commit -m "feat(cli): config show/path/set/unset over AppConfig JSON"
```

---

#### Task 13: Integration tests + README

**Files:**
- Create: `crates/xscore/tests/cli_smoke.rs`
- Modify: `README.md`

**Step 1: CLI smoke test**

```rust
// tests/cli_smoke.rs
use std::process::Command;

#[test]
fn xscore_help_exits_zero() {
    let out = Command::new(env!("CARGO_BIN_EXE_xscore")).arg("--help").output().unwrap();
    assert!(out.status.success());
    let s = String::from_utf8(out.stdout).unwrap();
    assert!(s.contains("connect"));
    assert!(s.contains("link"));
    assert!(s.contains("pane"));
}

#[test]
fn xscore_connect_creates_link_and_pane() {
    let out = Command::new(env!("CARGO_BIN_EXE_xscore"))
        .args(["connect", "shell:///bin/cat"]).output().unwrap();
    assert!(out.status.success());
    let s = String::from_utf8_lossy(&out.stdout);
    assert!(s.contains("link:"));
    assert!(s.contains("pane:"));

    let link_line = s.lines().find(|l| l.starts_with("link:")).unwrap();
    let pane_line = s.lines().find(|l| l.starts_with("pane:")).unwrap();
    let link_id = link_line.trim().strip_prefix("link:").unwrap().trim();
    let pane_id = pane_line.trim().strip_prefix("pane:").unwrap().trim();

    let list_out = Command::new(env!("CARGO_BIN_EXE_xscore"))
        .args(["link", "list"]).output().unwrap();
    assert!(String::from_utf8_lossy(&list_out.stdout).contains(link_id));

    let close_pane = Command::new(env!("CARGO_BIN_EXE_xscore"))
        .args(["pane", "close", "--id", pane_id]).output().unwrap();
    assert!(close_pane.status.success());

    let close_link = Command::new(env!("CARGO_BIN_EXE_xscore"))
        .args(["link", "close", "--id", link_id]).output().unwrap();
    assert!(close_link.status.success());
}

#[test]
fn xscore_connect_default_creates_two_links_for_same_url() {
    // First connect
    let out1 = Command::new(env!("CARGO_BIN_EXE_xscore"))
        .args(["connect", "shell:///bin/echo"]).output().unwrap();
    let id1 = String::from_utf8_lossy(&out1.stdout).lines()
        .find(|l| l.starts_with("link:")).unwrap().trim().strip_prefix("link:").unwrap().trim().to_string();

    // Second connect with same URL but no --link=existing → second link
    let out2 = Command::new(env!("CARGO_BIN_EXE_xscore"))
        .args(["connect", "shell:///bin/echo"]).output().unwrap();
    let id2 = String::from_utf8_lossy(&out2.stdout).lines()
        .find(|l| l.starts_with("link:")).unwrap().trim().strip_prefix("link:").unwrap().trim().to_string();

    assert_ne!(id1, id2, "two connect() calls should produce distinct link_ids");

    // Cleanup
    let _ = Command::new(env!("CARGO_BIN_EXE_xscore"))
        .args(["link", "close", "--id", &id1]).output();
    let _ = Command::new(env!("CARGO_BIN_EXE_xscore"))
        .args(["link", "close", "--id", &id2]).output();
}

#[test]
fn xscore_link_existing_unsupported_for_shell() {
    let out = Command::new(env!("CARGO_BIN_EXE_xscore"))
        .args(["connect", "shell:///bin/cat", "--link-existing"]).output().unwrap();
    assert!(!out.status.success(), "shell + --link=existing must fail");
    let s = String::from_utf8_lossy(&out.stderr);
    assert!(s.contains("not supported") || s.contains("LinkExistingUnsupported"),
            "expected LinkExistingUnsupported error, got: {s}");
}
```

**Step 2: README**

```markdown
# xscore

Rust session manager for SSH / shell / serial, modeled as **Link x Pane**.

- **Link** = one physical connection (SSH channel, serial port, local child, or tmux server).
- **Pane** = one working view into a link. N panes per link; each pane has its own id, snapshot, write state.

## Quick start

\`\`\`
xscore connect shell:///bin/bash
xscore connect ssh://user@host:22
xscore connect serial:///dev/ttyUSB0?baud=115200

# Multiple panes on the same link:
xscore connect ssh://u@h:22
xscore connect ssh://u@h:22 --link=existing   # opens a 2nd pane in the same tmux server
\`\`\`

## Commands

\`\`\`
xscore connect <url> [--save] [--link=existing]
xscore link list
xscore link show --id <link-id>
xscore link close --id <link-id>
xscore link save --id <link-id>
xscore link load --path <file>

xscore pane list [--link <link-id>]
xscore pane show --id <pane-id>
xscore pane close --id <pane-id>

xscore data read --pane <pane-id> [--bytes N]
xscore data write --pane <pane-id> [--from-file FILE]
xscore data resize --pane <pane-id> --cols C --rows R

xscore config show
xscore config path
xscore config set <key> <value>
xscore config unset <key>
\`\`\`

## Architecture

\`\`\`
CLI -> SessionManager
       +-- LinkRegistry     (address-keyed dedup)
       |    +-- Link { transport + sink + Vec<PaneId> }
       +-- PaneRegistry     (pane_id -> PaneInfo)
       +-- BackendRegistry  (pane_id -> Arc<dyn PaneBackend>)
                                  ^ wraps (Link, sink_handle)
\`\`\`

Each Link owns one Transport (raw IO) + one Sink (PTY/ANSI/tmux). Each Pane lives inside
one link and addresses the link's sink via a sink_handle (e.g. tmux `%42`). Closing a
pane does not close the link; closing a link drops every pane on it.

`--link=existing` opts into **address-keyed dedup**: search LinkRegistry for an existing
link matching the URL's transport address; if found, open a new pane on it; if not found,
error. Without `--link=existing`, every `xscore connect` creates a brand-new link.
```

**Step 4: Run all tests**

Run: `cargo test -p xscore`
Expected: all unit + integration tests pass.

**Step 5: Commit**

```bash
git add tests/ README.md
git commit -m "test+docs: Link x Pane smoke tests + README"
```

---

## Acceptance Criteria

- `cargo build -p xscore` → success
- `cargo test -p xscore` → all tests pass (unit + integration)
- `xscore --help` lists `connect`, `link`, `pane`, `data`, `config`
- `xscore connect shell:///bin/bash` creates a link + pane; stdout prints both ids
- `xscore connect <same url>` (no `--link=existing`) creates a **second** link (default = no dedup)
- `xscore connect <shell url> --link=existing` returns `Error::LinkExistingUnsupported("shell")`
- `xscore link list/show/close/save/load` over `LinkInfo`
- `xscore pane list/show/close` over `PaneInfo`
- `xscore data read/write/resize` over pane-id (NOT link-id)
- `xscore config show/path/set/unset` operates on `~/.config/xscore/config.json`
- Address-keyed dedup: `LinkAddress` Hash impl + `LinkRegistry::find_by_address` is the single source of truth for "is this a new link or attach"

## What's deferred (explicitly NOT in this slice)

- Real russh connect + pty channel (Task 8 skeleton; SSH-integration phase)
- Real serial port open (Task 8 skeleton; serial-integration phase)
- `Sink::attach()` for "remote" mode in TmuxSink (current no-op; will wire when SshTransport gets real IO)
- Live streaming subscribe (`PaneBackend::snapshot` only for now)
- Tmux per-char send-keys perf optimization
- Cross-process manager state (CLI = in-memory; MCP server = daemon later)
- MCP server, IPC server, web UI — land in subsequent slices once this surface stabilizes
- RawSink full implementation (`?tmux=off` today is a no-op; tracked in open question)
- `ssh + --link=existing` (returns no-match today; defer to SSH-integration phase)

## Open questions (will be revisited, do not block the slice)

1. **Link uniqueness across CLI invocations.** Today `LinkRegistry` is in-memory; restarting `xscore` forgets all links. Real dedup across runs requires a persistence layer (likely a daemon). For this slice, address-keyed dedup is **per-process**.
2. **`?tmux=off` semantics.** RawSink is a no-op stub for capture/write. Future PR will wire it to a passthrough mode.
3. **TmuxSink "auto" mode.** Resolves to `local` for shell/serial, `remote` for ssh. Future: explicit `tmux_socket` query param to override.
4. **Pane persistence.** Closing the CLI remounts the pane from tmux (via `list-panes`) — for recovery, but in this slice the CLI is in-memory only.

## Execution Handoff

This plan is saved to `doc/dev/roadmap/cli-first-implementation-plan.md`.

Next step: execute via `subagent-driven-development` skill — fresh subagent per task, two-stage review (spec compliance then code quality). Each task ends with `cargo test -p xscore` + `cargo build -p xscore` before commit.

Want me to proceed with execution, or revise the slice first?