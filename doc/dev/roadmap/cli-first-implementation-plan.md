# xscore CLI First Implementation Plan

**Goal:** Build the CLI-first slice of xscore — a Rust workspace that manages SSH / shell / serial sessions behind a single `Session` API, callable from `xscore` CLI today and (later) from MCP server and IPC server without code rewrite.

**Architecture (Transport × Sink):**
- Every session is the product of **one Transport** × **one Sink**.
  - **Transport** = raw IO transport: local process (`stdin/stdout`), serial port (`tokio-serial`), SSH (`russh` channel). Owns bytes only.
  - **Sink** = terminal/PTY semantics layer. First impl: `TmuxSink` which wraps an external tmux server via `tmux -L xscore ...`. Sink owns PTY sizing / line buffering / ANSI framing.
- `core::SessionManager` owns `Vec<(SessionId, Arc<dyn SessionBackend>)>` where `SessionBackend` is the **composite** of Transport + Sink. CLI calls `manager.create(...)`, `manager.read/write/resize`, etc. — the same methods MCP server will call next.
- tmux is always external, on a dedicated `xscore` socket (`tmux -L xscore ...`). For "ssh + tmux-in-ssh", tmux runs **inside the SSH channel** — we tell remote tmux to use its own server, no local tmux server needed for that transport.
- Config = `~/.config/xscore/config.json` + per-session JSON snippets under `~/.config/xscore/sessions/`.

**CLI entry:** `xscore connect <url> [--save]`. URLs:

```
xscore connect shell:///bin/bash
xscore connect serial:///dev/ttyUSB0?baud=115200&data_bits=8
xscore connect ssh://user@host:22?identity=~/.ssh/id_ed25519&proxy=ssh://jump@1.2.3.4
xscore connect ssh://user@host:22?tmux=on    # default
xscore connect ssh://user@host:22?tmux=off     # sink = RawSink — see open question
```

All other commands (`list`, `show`, `close`, `data read/write/resize`, `config ...`) take `--id <uuid>`. URLs are the entry point; `--from spec.json` is **removed**.

> ⚠️ **Open question (will not block the slice):** `serial + TmuxSink` — tmux expects a PTY, serial doesn't. The slice resolves this by running tmux locally and bridging serial bytes through tmux's pane input/output. Acceptable for CLI-first; revisit if a real "tmux attach to serial" demand surfaces.

**Tech Stack (locked from user):**
- CLI: `clap` v4 derive
- Async runtime: `tokio` (multi-thread, full features)
- SSH: `russh` (pure-Rust async)
- Shell: `tokio::process::Command`
- Serial: `tokio-serial`
- tmux: external `tmux` binary over CLI (`tmux -L xscore ...`)
- URL parsing: `url` crate
- Persistence: JSON files (config + per-session). No SQLite this phase.
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
│   │   ├── connect.rs                # xscore connect <url> [--save]
│   │   ├── session.rs                # session list/show/close/save/load
│   │   ├── data.rs                   # session data read/write/resize
│   │   └── config.rs                 # config show/path/set/unset
│   ├── core/
│   │   ├── mod.rs                    # pub SessionManager, re-exports
│   │   ├── manager.rs               # create/get/list/close + URL dispatch
│   │   ├── session.rs               # Session = Transport + Sink (struct)
│   │   ├── registry.rs               # in-memory Vec<(SessionId, SessionInfo)>
│   │   ├── id.rs                     # SessionId (uuid v7)
│   │   └── error.rs                  # thiserror enum (NotFound, Transport, Sink, ...)
│   ├── model/
│   │   ├── mod.rs
│   │   ├── session.rs                # ConnectSpec / SessionInfo / TransportConfig / SinkConfig
│   │   └── config.rs                 # AppConfig (default server, ssh defaults, ...)
│   ├── url/
│   │   ├── mod.rs                    # URL parser entry
│   │   ├── scheme.rs                 # shell | serial | ssh (placeholder)
│   │   └── connect.rs                # ConnectUrl → (TransportConfig, SinkConfig)
│   ├── transport/
│   │   ├── mod.rs                    # pub trait Transport
│   │   ├── factory.rs                # transport_for(TransportConfig) -> Box<dyn Transport>
│   │   ├── shell.rs                  # tokio::process::Command
│   │   ├── serial.rs                 # tokio-serial (stub, real open deferred)
│   │   └── ssh.rs                    # russh channel (stub, real open deferred)
│   ├── sink/
│   │   ├── mod.rs                    # pub trait Sink
│   │   ├── factory.rs                # sink_for(SinkConfig, transport_kind, transport) -> Box<dyn Sink>
│   │   ├── tmux.rs                   # TmuxSink (PTY/ANSI, wraps external tmux server)
│   │   └── raw.rs                    # RawSink (passthrough for tmux=off)
│   ├── backends/
│   │   ├── mod.rs                    # pub trait SessionBackend (composite)
│   │   └── composite.rs              # SessionBackend impl for (Transport + Sink)
│   ├── config/
│   │   ├── mod.rs
│   │   ├── loader.rs                 # load/save ConnectSpec JSON, xdg dirs
│   │   └── paths.rs                  # xdg path resolution
│   └── util/
│       ├── mod.rs
│       └── tmux_socket.rs            # socket name constant + helpers
└── tests/
    ├── cli_smoke.rs                  # spawn xscore binary, assert outputs
    ├── manager_mock.rs               # SessionManager with mockall Transport/Sink
    ├── url_parse.rs                  # URL → Transport/Sink config
    └── tmux_integration.rs           # real tmux -L xscore-test, gated by env
```

**Module rules (hard):**
- `cli` may depend on `core`, `model`, `url`, `config`. Never on `transport` / `sink` directly.
- `core` depends on `model`, `backends`, `sink::factory`, `transport::factory`. Never on `cli` or `config`.
- `backends` depends on `transport`, `sink`, `model`, `core::error`. Never on `cli`.
- `transport` depends on `model`, `core::error`. Never on `sink` or `cli`.
- `sink` depends on `model`, `core::error`, `transport` (to attach). Never on `cli`.
- `url` depends on `model`. Pure parsing layer, no I/O.
- `config` depends on `model`. Never on anything else.
- All cross-module state goes through `core::SessionManager` — no global statics.

**Why split `transport` and `sink` (vs the old single `backends/`):**

The old design treated each `SessionKind` as one backend. That broke the moment a session wants to combine layers — e.g. "ssh transport + tmux-in-ssh sink" (no local tmux), or "shell transport + local tmux sink" (local tmux). Splitting into orthogonal traits lets the same SessionManager serve all four combinations without combinator explosion:

| Transport → / Sink → | TmuxSink (local tmux) | TmuxSink (in-ssh tmux) | RawSink |
| --- | --- | --- | --- |
| Shell   | local shell, local tmux | n/a (local tmux already local) | passthrough |
| Serial  | serial over local tmux wrapper | n/a | raw serial |
| Ssh     | ssh over local tmux, ssh channel raw | ssh + run `tmux -L ssh <remote>` inside channel | raw ssh channel |

The "where the tmux server runs" decision is a **Sink config** (`TmuxSink::mode` = `local` / `remote` / `auto`), not a Transport config — that's the seam.

---

## Core Trait Design (preview, refined in Tasks 4–6)

```rust
// transport/mod.rs
#[async_trait]
pub trait Transport: Send + Sync {
    fn kind(&self) -> &'static str;
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
    /// Wire this sink to the running transport's IO.
    async fn attach(&mut self, transport: &dyn Transport) -> Result<()>;
    async fn write(&self, data: &[u8]) -> Result<usize>;
    /// Snapshot current displayed output (for tmux: capture-pane).
    async fn snapshot(&self, buf: &mut [u8]) -> Result<usize>;
    async fn resize(&self, cols: u16, rows: u16) -> Result<()>;
    async fn close(&self) -> Result<()>;
}

// backends/mod.rs
#[async_trait]
pub trait SessionBackend: Send + Sync {
    async fn snapshot(&self, buf: &mut [u8]) -> Result<usize>;  // → sink
    async fn write(&self, data: &[u8]) -> Result<usize>;        // → sink
    async fn write_raw(&self, data: &[u8]) -> Result<usize>;    // → transport
    async fn resize(&self, cols: u16, rows: u16) -> Result<()>;
    async fn close(&self) -> Result<()>;
}
```

`backends::composite::CompositeBackend` glues one Transport + one Sink into one `SessionBackend`. `SessionManager` only knows `SessionBackend`.

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

#### Task 2: Core error type and SessionId

**Objective:** Define `core::Error` (thiserror enum) and `core::SessionId` (uuid v7 newtype).

**Files:**
- Create: `crates/xscore/src/core/mod.rs`
- Create: `crates/xscore/src/core/id.rs`
- Create: `crates/xscore/src/core/error.rs`
- Create: `crates/xscore/src/core/{manager,registry,session}.rs` (empty placeholders)

**Step 1: Write failing test** (`crates/xscore/src/core/id.rs` top)

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_id_roundtrip_via_string() {
        let id = SessionId::new();
        let s = id.to_string();
        let parsed = SessionId::parse(&s).unwrap();
        assert_eq!(id, parsed);
    }

    #[test]
    fn session_id_parse_rejects_garbage() {
        assert!(SessionId::parse("not-a-uuid").is_err());
    }
}
```

**Step 2: Run test to verify failure**

Run: `cargo test -p xscore core::id`
Expected: FAIL — `SessionId` not defined.

**Step 3: Implement `SessionId`**

```rust
// core/id.rs
use serde::{Deserialize, Serialize};
use std::fmt;
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SessionId(Uuid);

impl SessionId {
    pub fn new() -> Self { Self(Uuid::now_v7()) }
    pub fn as_uuid(&self) -> Uuid { self.0 }
    pub fn parse(s: &str) -> Result<Self, uuid::Error> {
        Ok(Self(Uuid::parse(s)?))
    }
}

impl Default for SessionId {
    fn default() -> Self { Self::new() }
}

impl fmt::Display for SessionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}
```

**Step 4: Implement `core::Error`**

```rust
// core/error.rs
use thiserror::Error;
use super::id::SessionId;

#[derive(Debug, Error)]
pub enum Error {
    #[error("session not found: {0}")]
    NotFound(SessionId),

    #[error("session already exists: {0}")]
    AlreadyExists(String),

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

**Step 5: `core/mod.rs`**

```rust
pub mod error;
pub mod id;
pub mod manager;       // placeholder
pub mod registry;      // placeholder
pub mod session;       // placeholder

pub use error::{Error, Result};
pub use id::SessionId;
```

**Step 6: Run tests**

Run: `cargo test -p xscore`
Expected: 2 passed.

**Step 7: Commit**

```bash
git add crates/xscore/src/core/
git commit -m "feat(core): add SessionId (uuid v7), Error enum with Transport/Sink variants"
```

---

#### Task 3: URL → TransportConfig + SinkConfig parsing

**Objective:** Pure parser for `xscore connect <url>` → `ConnectSpec`. No I/O.

**Files:**
- Create: `crates/xscore/src/model/session.rs` (TransportConfig, SinkConfig, ConnectSpec, SessionInfo)
- Create: `crates/xscore/src/model/mod.rs`
- Create: `crates/xscore/src/url/mod.rs`
- Create: `crates/xscore/src/url/scheme.rs` (placeholder)
- Create: `crates/xscore/src/url/connect.rs`

**Step 1: Define config types** (in `model/session.rs`)

```rust
// model/session.rs
use serde::{Deserialize, Serialize};

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
    /// Tmux sink. `mode`: "local" | "remote" | "auto" | "off".
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
pub struct SessionInfo {
    pub id: super::super::core::SessionId,
    pub name: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub transport: TransportConfig,
    pub sink: SinkConfig,
}

impl SessionInfo {
    pub fn transport_str(&self) -> &'static str {
        match &self.transport {
            TransportConfig::Shell { .. } => "shell",
            TransportConfig::Serial { .. } => "serial",
            TransportConfig::Ssh { .. } => "ssh",
        }
    }
    pub fn sink_str(&self) -> &'static str {
        match &self.sink {
            SinkConfig::Tmux { .. } => "tmux",
        }
    }
}
```

**Step 2: Write failing test for URL parsing**

```rust
// url/connect.rs (test module)
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_ssh_url_with_tmux_on() {
        let s = parse("ssh://root@10.0.0.1:22?identity=~/.ssh/id_ed25519&tmux=on").unwrap();
        match s.transport {
            TransportConfig::Ssh { ref host, port, ref user, .. } => {
                assert_eq!(host, "10.0.0.1"); assert_eq!(port, 22); assert_eq!(user, "root");
            }
            _ => panic!("expected ssh"),
        }
        match s.sink { SinkConfig::Tmux { mode, .. } => assert_eq!(mode, "auto"), }
    }

    #[test]
    fn parse_shell_url() {
        let s = parse("shell:///bin/bash").unwrap();
        assert!(matches!(s.transport, TransportConfig::Shell { .. }));
    }

    #[test]
    fn parse_serial_url_with_query() {
        let s = parse("serial:///dev/ttyUSB0?baud=115200&data_bits=8").unwrap();
        match s.transport {
            TransportConfig::Serial { ref path, baud, data_bits, .. } => {
                assert_eq!(path, "/dev/ttyUSB0"); assert_eq!(baud, 115200); assert_eq!(data_bits, 8);
            }
            _ => panic!("expected serial"),
        }
    }

    #[test]
    fn parse_rejects_unknown_scheme() {
        assert!(parse("telnet://host").is_err());
    }

    #[test]
    fn tmux_query_overrides_sink_mode() {
        let s = parse("ssh://u@h:22?tmux=off").unwrap();
        match s.sink { SinkConfig::Tmux { mode, .. } => assert_eq!(mode, "off"), }
    }
}
```

**Step 3: Implement URL parser**

```rust
// url/connect.rs
use crate::core::{Error, Result};
use crate::model::{ConnectSpec, SinkConfig, TransportConfig};
use std::collections::HashMap;

pub fn parse(s: &str) -> Result<ConnectSpec> {
    let url = url::Url::parse(s).map_err(|e| Error::Url(e.to_string()))?;
    let scheme = url.scheme();
    let host = url.host_str().map(|s| s.to_string());
    let user = (!url.username().is_empty()).then(|| url.username().to_string());

    let mut pairs: HashMap<String, String> = url.query_pairs()
        .map(|(k, v)| (k.into_owned(), v.into_owned())).collect();
    let tmux_mode = pairs.remove("tmux").unwrap_or_else(|| "auto".into());
    let name = pairs.remove("name").unwrap_or_default();

    let transport = match scheme {
        "shell" | "sh" => {
            let path = url.path().trim_start_matches('/').to_string();
            if path.is_empty() { return Err(Error::Url("shell URL needs command".into())); }
            TransportConfig::Shell { command: path, args: vec![], cwd: None }
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
            TransportConfig::Serial { path, baud, data_bits, parity, stop_bits }
        }
        "ssh" => {
            let host = host.ok_or_else(|| Error::Url("ssh URL needs host".into()))?;
            let user = user.ok_or_else(|| Error::Url("ssh URL needs user".into()))?;
            let port = url.port().unwrap_or(22);
            let identity = pairs.remove("identity");
            let proxy = pairs.remove("proxy");
            TransportConfig::Ssh { host, port, user, identity, proxy }
        }
        other => return Err(Error::Url(format!("unknown scheme: {other}"))),
    };

    let sink = SinkConfig::Tmux {
        mode: tmux_mode,
        session_name: (!name.is_empty()).then(|| name.clone()),
    };

    Ok(ConnectSpec { name, transport, sink })
}
```

```rust
// url/mod.rs
pub mod connect;
pub mod scheme;

pub use connect::parse;
```

```rust
// url/scheme.rs  (placeholder for future per-scheme validation)
// (empty)
```

**Step 4: Update `model/mod.rs`** to export.

```rust
pub mod session;
pub mod config;       // placeholder

pub use session::{ConnectSpec, SessionInfo, SinkConfig, TransportConfig};
```

**Step 5: Run tests**

Run: `cargo test -p xscore url`
Expected: 5 passing.

**Step 6: Commit**

```bash
git add crates/xscore/src/url/ crates/xscore/src/model/
git commit -m "feat(url): parse connect URL into TransportConfig + SinkConfig"
```

---

#### Task 4: Transport trait + echo stub

**Objective:** Define `Transport` async trait with `open/write/read/resize/close`. Echo impl for tests.

**Files:**
- Create: `crates/xscore/src/transport/mod.rs`
- Create: `crates/xscore/src/transport/{shell,serial,ssh}.rs` (stubs)
- Create: `crates/xscore/src/transport/factory.rs` (placeholder)

**Step 1: Write failing test**

```rust
// transport/mod.rs (test module)
#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;

    struct Echo;

    #[async_trait]
    impl Transport for Echo {
        fn kind(&self) -> &'static str { "echo" }
        async fn open(&mut self) -> Result<()> { Ok(()) }
        async fn write(&self, data: &[u8]) -> Result<usize> { Ok(data.len()) }
        async fn read(&self, _buf: &mut [u8]) -> Result<usize> { Ok(0) }
        async fn resize(&self, _: u16, _: u16) -> Result<()> { Ok(()) }
        async fn close(&self) -> Result<()> { Ok(()) }
    }

    #[tokio::test]
    async fn echo_transport_writes_through() {
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
use crate::core::Result;
use async_trait::async_trait;

#[async_trait]
pub trait Transport: Send + Sync {
    fn kind(&self) -> &'static str;
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

Stub modules contain `// Transport stub` only.

**Step 3: Run tests**

Run: `cargo test -p xscore transport`
Expected: 1 passing.

**Step 4: Commit**

```bash
git add crates/xscore/src/transport/
git commit -m "feat(transport): Transport async trait + echo stub"
```

---

#### Task 5: Sink trait + tmux + raw + factory

**Objective:** Define `Sink` trait; implement TmuxSink on external tmux socket; RawSink stub for `tmux=off`. Factory decides mode (local vs remote vs raw) from `SinkConfig` + transport kind.

**Files:**
- Create: `crates/xscore/src/sink/mod.rs`
- Create: `crates/xscore/src/sink/tmux.rs`
- Create: `crates/xscore/src/sink/raw.rs`
- Create: `crates/xscore/src/sink/factory.rs`
- Create: `crates/xscore/src/util/mod.rs`
- Create: `crates/xscore/src/util/tmux_socket.rs`

**Step 1: Write failing test for tmux_socket helper**

```rust
// util/tmux_socket.rs (test)
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_socket_is_xscore() {
        let prev = std::env::var("XSCORE_SOCKET").ok();
        std::env::remove_var("XSCORE_SOCKET");
        assert_eq!(socket_name(), "xscore");
        if let Some(v) = prev { std::env::set_var("XSCORE_SOCKET", v); }
    }

    #[test]
    fn override_socket_via_env() {
        std::env::set_var("XSCORE_SOCKET", "xscore-test");
        assert_eq!(socket_name(), "xscore-test");
        std::env::remove_var("XSCORE_SOCKET");
    }
}
```

**Step 2: Implement socket helper**

```rust
// util/tmux_socket.rs
pub const DEFAULT_SOCKET: &str = "xscore";

pub fn socket_name() -> String {
    std::env::var("XSCORE_SOCKET").unwrap_or_else(|_| DEFAULT_SOCKET.to_string())
}

pub fn tmux_arg_socket() -> Vec<String> {
    vec!["-L".into(), socket_name()]
}
```

`util/mod.rs`:

```rust
pub mod tmux_socket;
pub use tmux_socket::{socket_name as _socket_name_impl};
pub use tmux_socket::tmux_arg_socket;
```

(Note: `socket_name` re-export collides if used at sink/util boundary — keep consumers referring to `tmux_arg_socket` only.)

**Step 3: Write failing test for Sink trait**

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
        async fn attach(&mut self, _t: &dyn Transport) -> Result<()> { Ok(()) }
        async fn write(&self, data: &[u8]) -> Result<usize> { Ok(data.len()) }
        async fn snapshot(&self, _buf: &mut [u8]) -> Result<usize> { Ok(0) }
        async fn resize(&self, _: u16, _: u16) -> Result<()> { Ok(()) }
        async fn close(&self) -> Result<()> { Ok(()) }
    }

    #[tokio::test]
    async fn noop_sink_attaches_and_writes() {
        let mut s = Noop;
        let t = crate::transport::tests_only::EchoStub;
        s.attach(&t).await.unwrap();
        assert_eq!(s.write(b"x").await.unwrap(), 1);
    }
}
```

(For `crate::transport::tests_only::EchoStub` see the dev-helper note in step 4.)

**Step 4: Implement Sink trait**

```rust
// sink/mod.rs
use crate::core::Result;
use crate::transport::Transport;
use async_trait::async_trait;

#[async_trait]
pub trait Sink: Send + Sync {
    fn kind(&self) -> &'static str;
    async fn attach(&mut self, transport: &dyn Transport) -> Result<()>;
    async fn write(&self, data: &[u8]) -> Result<usize>;
    async fn snapshot(&self, buf: &mut [u8]) -> Result<usize>;
    async fn resize(&self, cols: u16, rows: u16) -> Result<()>;
    async fn close(&self) -> Result<()>;
}

pub mod factory;
pub mod raw;
pub mod tmux;
```

**Step 5: TmuxSink**

```rust
// sink/tmux.rs
use crate::core::{Error, Result};
use crate::transport::Transport;
use crate::util::tmux_arg_socket;
use async_trait::async_trait;
use tokio::process::Command;

pub struct TmuxSink {
    pub mode: String,           // "local" | "remote" (post-resolution)
    pub session_name: String,
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
}

#[async_trait]
impl super::Sink for TmuxSink {
    fn kind(&self) -> &'static str { "tmux" }

    async fn attach(&mut self, _transport: &dyn Transport) -> Result<()> {
        // Wiring is done in create(); attach is reserved for future sink modes that
        // need transport IO after the fact (e.g. ssh channel gets tmux ssh channel pipe).
        Ok(())
    }

    async fn write(&self, data: &[u8]) -> Result<usize> {
        let s = std::str::from_utf8(data).map_err(|e| Error::Invalid(e.to_string()))?;
        let mut cmd = Command::new("tmux");
        cmd.args(tmux_arg_socket());
        cmd.arg("send-keys").arg("-t").arg(&self.session_name);
        for ch in s.chars() { cmd.arg(ch.to_string()); }
        let status = cmd.status().await?;
        if !status.success() { return Err(Error::Sink(format!("send-keys exit {status}"))); }
        Ok(data.len())
    }

    async fn snapshot(&self, buf: &mut [u8]) -> Result<usize> {
        let out = Command::new("tmux")
            .args(tmux_arg_socket())
            .args(["capture-pane", "-p", "-t", &self.session_name, "-J"])
            .output().await?;
        if !out.status.success() {
            return Err(Error::Sink(format!("capture-pane exit {:?}", out.status)));
        }
        let bytes = &out.stdout;
        let n = bytes.len().min(buf.len());
        buf[..n].copy_from_slice(&bytes[..n]);
        Ok(n)
    }

    async fn resize(&self, cols: u16, rows: u16) -> Result<()> {
        let status = Command::new("tmux")
            .args(tmux_arg_socket())
            .args(["resize-window", "-t", &self.session_name,
                   "-x", &cols.to_string(), "-y", &rows.to_string()])
            .status().await?;
        if !status.success() { return Err(Error::Sink(format!("resize exit {status}"))); }
        Ok(())
    }

    async fn close(&self) -> Result<()> {
        let _ = Command::new("tmux")
            .args(tmux_arg_socket())
            .args(["kill-session", "-t", &self.session_name])
            .status().await?;
        Ok(())
    }
}
```

**Step 6: RawSink** (`sink/raw.rs`)

```rust
// sink/raw.rs
use crate::core::Result;
use crate::transport::Transport;
use async_trait::async_trait;

/// Passthrough sink for `tmux=off` (or auto-resolved to off). Today a no-op
/// stub; future PR wires direct transport I/O passthrough.
pub struct RawSink;

#[async_trait]
impl super::Sink for RawSink {
    fn kind(&self) -> &'static str { "raw" }
    async fn attach(&mut self, _: &dyn Transport) -> Result<()> { Ok(()) }
    async fn write(&self, _: &[u8]) -> Result<usize> { Ok(0) }
    async fn snapshot(&self, _: &mut [u8]) -> Result<usize> { Ok(0) }
    async fn resize(&self, _: u16, _: u16) -> Result<()> { Ok(()) }
    async fn close(&self) -> Result<()> { Ok(()) }
}
```

**Step 7: Factory** (`sink/factory.rs`)

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

Add `pub mod factory;` in `sink/mod.rs`.

**Step 8: Run tests**

Run: `cargo test -p xscore sink:: util::`
Expected: sink test + util tests pass.

**Step 9: Commit**

```bash
git add crates/xscore/src/sink/ crates/xscore/src/util/
git commit -m "feat(sink): Sink trait + TmuxSink + RawSink + sink_for factory"
```

---

#### Task 6: Composite SessionBackend + SessionManager

**Objective:** Glue Transport + Sink into one `SessionBackend` (for SessionManager). SessionManager uses the composite, never sees the layers.

**Files:**
- Create: `crates/xscore/src/core/session.rs`
- Create: `crates/xscore/src/backends/mod.rs`
- Create: `crates/xscore/src/backends/composite.rs`
- Create: `crates/xscore/src/transport/factory.rs` (real impl, replaces placeholder)
- Modify: `crates/xscore/src/core/registry.rs`
- Modify: `crates/xscore/src/core/manager.rs`
- Modify: `crates/xscore/src/core/mod.rs`

**Step 1: Define `core::Session`** (`core/session.rs`)

```rust
// core/session.rs
use crate::transport::Transport;
use crate::sink::Sink;

pub struct Session {
    pub transport: Box<dyn Transport>,
    pub sink: Box<dyn Sink>,
}
```

**Step 2: SessionBackend trait** (`backends/mod.rs`)

```rust
// backends/mod.rs
use crate::core::Result;
use async_trait::async_trait;

#[async_trait]
pub trait SessionBackend: Send + Sync {
    /// Snapshot current displayed output (composite: sink first).
    async fn snapshot(&self, buf: &mut [u8]) -> Result<usize>;

    /// Write bytes downstream via sink (PTY/ANSI semantics).
    async fn write(&self, data: &[u8]) -> Result<usize>;

    /// Forward bytes upstream directly to the transport (bypass sink).
    async fn write_raw(&self, data: &[u8]) -> Result<usize>;

    async fn resize(&self, cols: u16, rows: u16) -> Result<()>;
    async fn close(&self) -> Result<()>;
}

pub mod composite;
```

**Step 3: Composite impl** (`backends/composite.rs`)

```rust
// backends/composite.rs
use crate::core::session::Session;
use crate::core::Result;
use async_trait::async_trait;
use std::sync::Arc;
use tokio::sync::Mutex;

pub struct CompositeBackend {
    inner: Arc<Mutex<Session>>,
}

impl CompositeBackend {
    pub fn new(session: Session) -> Self {
        Self { inner: Arc::new(Mutex::new(session)) }
    }
}

#[async_trait]
impl super::SessionBackend for CompositeBackend {
    async fn snapshot(&self, buf: &mut [u8]) -> Result<usize> {
        let g = self.inner.lock().await;
        g.sink.snapshot(buf).await
    }

    async fn write(&self, data: &[u8]) -> Result<usize> {
        let g = self.inner.lock().await;
        g.sink.write(data).await
    }

    async fn write_raw(&self, data: &[u8]) -> Result<usize> {
        let g = self.inner.lock().await;
        g.transport.write(data).await
    }

    async fn resize(&self, cols: u16, rows: u16) -> Result<()> {
        let g = self.inner.lock().await;
        g.transport.resize(cols, rows).await?;
        g.sink.resize(cols, rows).await?;
        Ok(())
    }

    async fn close(&self) -> Result<()> {
        let mut g = self.inner.lock().await;
        let _ = g.sink.close().await;
        g.transport.close().await?;
        Ok(())
    }
}
```

**Step 4: Registry** (`core/registry.rs`)

```rust
// core/registry.rs
use std::sync::Arc;
use tokio::sync::RwLock;
use super::id::SessionId;
use crate::model::SessionInfo;

#[derive(Default)]
pub struct Registry {
    inner: RwLock<Vec<(SessionId, SessionInfo)>>,
}

impl Registry {
    pub fn new() -> Self { Self::default() }

    pub async fn insert(&self, id: SessionId, info: SessionInfo) { self.inner.write().await.push((id, info)); }
    pub async fn list(&self) -> Vec<(SessionId, SessionInfo)> { self.inner.read().await.clone() }
    pub async fn get(&self, id: SessionId) -> Option<SessionInfo> {
        self.inner.read().await.iter().find(|(k, _)| *k == id).map(|(_, v)| v.clone())
    }
    pub async fn remove(&self, id: SessionId) -> Option<SessionInfo> {
        let mut g = self.inner.write().await;
        let pos = g.iter().position(|(k, _)| *k == id)?;
        Some(g.remove(pos).1)
    }
}

pub type ArcRegistry = Arc<Registry>;
```

**Step 5: SessionManager** (`core/manager.rs`)

```rust
// core/manager.rs
use std::sync::Arc;
use super::{Error, Result, SessionId};
use super::registry::Registry;
use super::session::Session as CoreSession;
use crate::backends::SessionBackend;
use crate::backends::composite::CompositeBackend;
use crate::model::{ConnectSpec, SessionInfo};
use crate::sink::factory::sink_for;
use crate::transport::factory as transport_factory;

pub struct SessionManager {
    backends: tokio::sync::RwLock<Vec<(SessionId, Arc<dyn SessionBackend>)>>,
    registry: Arc<Registry>,
}

impl Default for SessionManager {
    fn default() -> Self { Self::new() }
}

impl SessionManager {
    pub fn new() -> Self {
        Self {
            backends: tokio::sync::RwLock::new(Vec::new()),
            registry: Arc::new(Registry::new()),
        }
    }

    pub async fn create(&self, spec: ConnectSpec) -> Result<SessionId> {
        let transport_kind_for_factory = match &spec.transport {
            crate::model::TransportConfig::Shell { .. } => "shell",
            crate::model::TransportConfig::Serial { .. } => "serial",
            crate::model::TransportConfig::Ssh { .. } => "ssh",
        };

        let mut transport = transport_factory::transport_for(spec.transport.clone()).await?;
        let sink_kind_hint = transport.kind();
        let sink = sink_for(&spec.sink, transport_kind_for_factory, transport.as_mut()).await?;
        // attach is a no-op for current sinks but kept for forward-compat
        let mut transport: Box<dyn crate::transport::Transport> = transport;
        sink.attach(transport.as_mut()).await?;

        let session = CoreSession { transport, sink: sink };
        let id = SessionId::new();
        let info = SessionInfo {
            id,
            name: if spec.name.is_empty() { id.to_string() } else { spec.name.clone() },
            created_at: chrono::Utc::now(),
            transport: spec.transport.clone(),
            sink: spec.sink.clone(),
        };
        let composite: Arc<dyn SessionBackend> = Arc::new(CompositeBackend::new(session));
        self.backends.write().await.push((id, composite));
        self.registry.insert(id, info).await;
        Ok(id)
    }

    pub async fn get(&self, id: SessionId) -> Result<SessionInfo> {
        self.registry.get(id).await.ok_or(Error::NotFound(id))
    }

    pub async fn list(&self) -> Vec<(SessionId, SessionInfo)> {
        self.registry.list().await
    }

    pub async fn backend(&self, id: SessionId) -> Result<Arc<dyn SessionBackend>> {
        let g = self.backends.read().await;
        g.iter().find(|(k, _)| *k == id).map(|(_, b)| b.clone())
            .ok_or(Error::NotFound(id))
    }

    pub async fn close(&self, id: SessionId) -> Result<()> {
        let backend = self.backend(id).await?;
        backend.close().await?;
        self.backends.write().await.retain(|(k, _)| *k != id);
        self.registry.remove(id).await;
        Ok(())
    }
}
```

**Step 6: Transport factory** (`transport/factory.rs`)

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

**Step 7: Update `core/mod.rs`**

```rust
pub mod error;
pub mod id;
pub mod manager;
pub mod registry;
pub mod session;

pub use error::{Error, Result};
pub use id::SessionId;
```

**Step 8: Run tests**

Run: `cargo test -p xscore`
Expected: existing tests still pass.

**Step 9: Commit**

```bash
git add crates/xscore/src/core/ crates/xscore/src/backends/ crates/xscore/src/transport/factory.rs crates/xscore/src/sink/
git commit -m "feat(core): SessionManager wires Transport + Sink via CompositeBackend"
```

---

#### Task 7: ShellTransport (tokio::process::Command)

**Objective:** Concrete `Transport` impl that spawns a local child process with stdin/stdout pipes.

**Files:**
- Modify: `crates/xscore/src/transport/shell.rs`

**Step 1: Write failing test**

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

**Step 2: Implement**

```rust
// transport/shell.rs
use crate::core::{Error, Result};
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

    async fn resize(&self, _: u16, _: u16) -> Result<()> {
        // ShellTransport without pty: resize is a no-op. For pty, switch to portable-pty.
        Ok(())
    }

    async fn close(&self) -> Result<()> {
        let mut g = self.child.lock().await;
        if let Some(c) = g.as_mut() { c.kill().await?; }
        *g = None;
        Ok(())
    }
}
```

**Step 3: Run tests**

Run: `cargo test -p xscore transport::shell`
Expected: 1 passing.

**Step 4: Commit**

```bash
git add crates/xscore/src/transport/shell.rs
git commit -m "feat(transport/shell): tokio::process::Command Transport impl"
```

---

#### Task 8: SerialTransport (tokio-serial) — stub open

**Objective:** Concrete `Transport` impl skeleton; real port open deferred to integration phase.

**Files:**
- Modify: `crates/xscore/src/transport/serial.rs`

```rust
// transport/serial.rs
use crate::core::{Error, Result};
use async_trait::async_trait;

pub struct SerialTransport {
    pub path: String,
    pub baud: u32,
    pub data_bits: u8,
    pub parity: String,
    pub stop_bits: u8,
}

impl SerialTransport {
    pub async fn new(path: String, baud: u32, data_bits: u8, parity: String, stop_bits: u8) -> Result<Self> {
        Ok(Self { path, baud, data_bits, parity, stop_bits })
    }
}

#[async_trait]
impl super::Transport for SerialTransport {
    fn kind(&self) -> &'static str { "serial" }
    async fn open(&mut self) -> Result<()> {
        // tokio-serial integration deferred to serial-port integration phase.
        Err(Error::Transport("SerialTransport::open pending integration phase".into()))
    }
    async fn write(&self, _: &[u8]) -> Result<usize> { Err(Error::Transport("not open".into())) }
    async fn read(&self, _: &mut [u8]) -> Result<usize> { Err(Error::Transport("not open".into())) }
    async fn resize(&self, _: u16, _: u16) -> Result<()> { Ok(()) }
    async fn close(&self) -> Result<()> { Ok(()) }
}
```

**Step 1: Compile**

Run: `cargo build -p xscore`. Expected: success.

**Step 2: Commit**

```bash
git add crates/xscore/src/transport/serial.rs
git commit -m "feat(transport/serial): SerialTransport skeleton (open deferred)"
```

---

#### Task 9: SshTransport (russh) — stub

**Objective:** Concrete `Transport` impl skeleton; real russh channel deferred.

**Files:**
- Modify: `crates/xscore/src/transport/ssh.rs`

```rust
// transport/ssh.rs
use crate::core::{Error, Result};
use async_trait::async_trait;

pub struct SshTransport {
    pub host: String,
    pub port: u16,
    pub user: String,
    pub identity: Option<String>,
    pub proxy: Option<String>,
}

impl SshTransport {
    pub async fn connect(host: String, port: u16, user: String, identity: Option<String>, proxy: Option<String>) -> Result<Self> {
        Ok(Self { host, port, user, identity, proxy })
    }
}

#[async_trait]
impl super::Transport for SshTransport {
    fn kind(&self) -> &'static str { "ssh" }
    async fn open(&mut self) -> Result<()> {
        Err(Error::Transport("SshTransport::open pending integration phase".into()))
    }
    async fn write(&self, _: &[u8]) -> Result<usize> { Err(Error::Transport("not open".into())) }
    async fn read(&self, _: &mut [u8]) -> Result<usize> { Err(Error::Transport("not open".into())) }
    async fn resize(&self, _: u16, _: u16) -> Result<()> { Ok(()) }
    async fn close(&self) -> Result<()> { Ok(()) }
}
```

**Step 1: Compile**

Run: `cargo build -p xscore`. Expected: success.

**Step 2: Commit**

```bash
git add crates/xscore/src/transport/ssh.rs
git commit -m "feat(transport/ssh): SshTransport skeleton (open deferred)"
```

---

#### Task 10: Config loader (XDG paths + ConnectSpec JSON)

**Objective:** Resolve `~/.config/xscore/config.json` + per-session `~/.config/xscore/sessions/<uuid>.json`. Save + load `ConnectSpec`.

**Files:**
- Create: `crates/xscore/src/config/mod.rs`
- Create: `crates/xscore/src/config/paths.rs`
- Create: `crates/xscore/src/config/loader.rs`

**Step 1: paths.rs**

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
pub fn session_path(id: &str) -> std::path::PathBuf { sessions_dir().join(format!("{id}.json")) }
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
pub use paths::{config_dir, config_file, ensure_dirs, session_path, sessions_dir};
```

**Step 2: Commit**

```bash
git add crates/xscore/src/config/
git commit -m "feat(config): XDG paths + ConnectSpec save/load"
```

---

#### Task 11: CLI — clap skeleton + `connect` subcommand

**Objective:** `xscore connect <url> [--save]` works end-to-end against a real local tmux session.

**Files:**
- Create: `crates/xscore/src/cli/mod.rs`
- Create: `crates/xscore/src/cli/connect.rs`
- Create: `crates/xscore/src/cli/session.rs` (placeholder)
- Create: `crates/xscore/src/cli/data.rs` (placeholder)
- Create: `crates/xscore/src/cli/config.rs` (placeholder)
- Modify: `crates/xscore/src/main.rs`

**Step 1: Cli struct**

```rust
// cli/mod.rs
pub mod config;
pub mod data;
pub mod session;
pub mod connect;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "xscore", version, about = "Manage SSH / shell / serial sessions")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Subcommand)]
pub enum Commands {
    /// Open a session from a URL
    Connect(connect::ConnectArgs),
    /// List / show / close / save / load sessions
    Session(session::SessionCmd),
    /// Read / write / resize data on a live session
    Data(data::DataCmd),
    /// Show / edit config values
    Config(config::ConfigCmd),
}
```

```rust
// cli/connect.rs
use clap::Args;
use std::sync::Arc;

#[derive(Args)]
pub struct ConnectArgs {
    /// URL: shell://, ssh://, serial://
    pub url: String,

    /// Save the parsed spec to ~/.config/xscore/sessions/<id>.json
    #[arg(long)]
    pub save: bool,
}

pub async fn run(mgr: Arc<SessionManager>, args: ConnectArgs) -> anyhow::Result<()> {
    let spec = xscore::url::parse(&args.url)?;
    let id = mgr.create(spec.clone()).await?;
    println!("created: {id}");
    if args.save {
        xscore::config::ensure_dirs()?;
        xscore::config::save_connect_spec(
            &xscore::config::session_path(&id.to_string()), &spec)?;
    }
    Ok(())
}
```

**Step 2: main.rs**

```rust
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();
    let cli = xscore::cli::Cli::parse();
    let mgr = std::sync::Arc::new(xscore::core::SessionManager::new());
    match cli.command {
        xscore::cli::Commands::Connect(c) => xscore::cli::connect::run(mgr, c).await?,
        xscore::cli::Commands::Session(_c) => println!("session (todo)"),
        xscore::cli::Commands::Data(_c) => println!("data (todo)"),
        xscore::cli::Commands::Config(_c) => println!("config (todo)"),
    }
    Ok(())
}
```

**Step 3: Smoke**

```bash
cargo run -p xscore -- connect 'shell:///bin/cat' --save
tmux -L xscore ls
```

Expected: `created: <uuid>` printed; tmux session `xscore-<uuid>` exists in `tmux -L xscore ls`.

**Step 4: Commit**

```bash
git add crates/xscore/src/cli/ crates/xscore/src/main.rs
git commit -m "feat(cli): xscore connect <url> opens a session via URL + factory"
```

---

#### Task 12: CLI `session` subcommand — list/show/close/save/load

**Objective:** Wire `xscore session list/show/close/save/load` to `SessionManager` + `config::loader`.

**Files:**
- Modify: `crates/xscore/src/cli/session.rs`

```rust
// cli/session.rs
use crate::core::SessionId;
use crate::core::SessionManager;
use clap::Subcommand;
use std::sync::Arc;

#[derive(Subcommand)]
pub enum SessionCmd {
    List,
    Show { #[arg(long)] id: String },
    Close { #[arg(long)] id: String },
    Save { #[arg(long)] id: String },
    Load { #[arg(long)] path: String },
}

pub async fn run(mgr: Arc<SessionManager>, cmd: SessionCmd) -> anyhow::Result<()> {
    match cmd {
        SessionCmd::List => {
            for (id, info) in mgr.list().await {
                println!("{id}\t{}\t{}\t{}", info.transport_str(), info.sink_str(), info.name);
            }
        }
        SessionCmd::Show { id } => {
            let sid = SessionId::parse(&id)?;
            let info = mgr.get(sid).await?;
            println!("{}", serde_json::to_string_pretty(&info)?);
        }
        SessionCmd::Close { id } => {
            let sid = SessionId::parse(&id)?;
            mgr.close(sid).await?;
            println!("closed: {sid}");
        }
        SessionCmd::Save { id } => {
            let sid = SessionId::parse(&id)?;
            let info = mgr.get(sid).await?;
            let spec = xscore::model::ConnectSpec {
                name: info.name.clone(),
                transport: info.transport.clone(),
                sink: info.sink.clone(),
            };
            xscore::config::ensure_dirs()?;
            xscore::config::save_connect_spec(
                &xscore::config::session_path(&sid.to_string()), &spec)?;
            println!("saved: {sid}");
        }
        SessionCmd::Load { path } => {
            let spec = xscore::config::load_connect_spec(std::path::Path::new(&path))?;
            println!("{}", serde_json::to_string_pretty(&spec)?);
        }
    }
    Ok(())
}
```

**Step 2: Wire to main**

```rust
match cli.command {
    xscore::cli::Commands::Connect(c) => xscore::cli::connect::run(mgr.clone(), c).await?,
    xscore::cli::Commands::Session(c) => xscore::cli::session::run(mgr, c).await?,
    xscore::cli::Commands::Data(_c) => println!("data (todo)"),
    xscore::cli::Commands::Config(_c) => println!("config (todo)"),
}
```

**Step 3: Smoke**

```bash
ID=$(cargo run -p xscore -- session list | awk '{print $1}' | head -1)
cargo run -p xscore -- session show --id "$ID"
cargo run -p xscore -- session close --id "$ID"
cargo run -p xscore -- session list   # should be empty
```

**Step 4: Commit**

```bash
git add crates/xscore/src/cli/session.rs crates/xscore/src/main.rs
git commit -m "feat(cli): session list/show/close/save/load"
```

---

#### Task 13: CLI `data` subcommand — read/write/resize

**Objective:** `xscore data write --id <id> [--from-file FILE]`, `xscore data read --id <id> [--bytes N]`, `xscore data resize --id <id> --cols 120 --rows 30`.

**Files:**
- Modify: `crates/xscore/src/cli/data.rs`

```rust
// cli/data.rs
use crate::core::SessionId;
use crate::core::SessionManager;
use clap::Subcommand;
use std::sync::Arc;

#[derive(Subcommand)]
pub enum DataCmd {
    Read { #[arg(long)] id: String, #[arg(long, default_value_t = 4096)] bytes: usize },
    Write { #[arg(long)] id: String, #[arg(long)] from_file: Option<String> },
    Resize { #[arg(long)] id: String, #[arg(long)] cols: u16, #[arg(long)] rows: u16 },
}

pub async fn run(mgr: Arc<SessionManager>, cmd: DataCmd) -> anyhow::Result<()> {
    match cmd {
        DataCmd::Read { id, bytes } => {
            let sid = SessionId::parse(&id)?;
            let backend = mgr.backend(sid).await?;
            let mut buf = vec![0u8; bytes];
            let n = backend.snapshot(&mut buf).await?;
            std::io::Write::write_all(&mut std::io::stdout(), &buf[..n])?;
        }
        DataCmd::Write { id, from_file } => {
            let sid = SessionId::parse(&id)?;
            let backend = mgr.backend(sid).await?;
            let data = if let Some(p) = from_file { std::fs::read(p)? } else {
                let mut s = String::new();
                std::io::Read::read_to_string(&mut std::io::stdin(), &mut s)?;
                s.into_bytes()
            };
            let n = backend.write(&data).await?;
            eprintln!("wrote {n} bytes");
        }
        DataCmd::Resize { id, cols, rows } => {
            let sid = SessionId::parse(&id)?;
            let backend = mgr.backend(sid).await?;
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
    xscore::cli::Commands::Connect(c) => xscore::cli::connect::run(mgr.clone(), c).await?,
    xscore::cli::Commands::Session(c) => xscore::cli::session::run(mgr.clone(), c).await?,
    xscore::cli::Commands::Data(c) => xscore::cli::data::run(mgr, c).await?,
    xscore::cli::Commands::Config(_c) => println!("config (todo)"),
}
```

**Step 3: Smoke**

```bash
cargo run -p xscore -- connect 'shell:///bin/cat' --save
ID=$(cargo run -p xscore -- session list | awk '{print $1}' | head -1)
echo "ping" | cargo run -p xscore -- data write --id "$ID"
cargo run -p xscore -- data resize --id "$ID" --cols 200 --rows 50
cargo run -p xscore -- data read --id "$ID" --bytes 4096
cargo run -p xscore -- session close --id "$ID"
```

**Step 4: Commit**

```bash
git add crates/xscore/src/cli/data.rs crates/xscore/src/main.rs
git commit -m "feat(cli): data read/write/resize over composite SessionBackend"
```

---

#### Task 14: CLI `config` subcommand

**Objective:** `xscore config show/path/set/unset` over `AppConfig`.

**Files:**
- Modify: `crates/xscore/src/cli/config.rs`
- Modify: `crates/xscore/src/model/config.rs`

**Step 1: AppConfig** (`model/config.rs`)

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

**Step 2: config CLI** (`cli/config.rs`)

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

**Step 3: Wire to main**

```rust
match cli.command {
    xscore::cli::Commands::Connect(c) => xscore::cli::connect::run(mgr.clone(), c).await?,
    xscore::cli::Commands::Session(c) => xscore::cli::session::run(mgr.clone(), c).await?,
    xscore::cli::Commands::Data(c) => xscore::cli::data::run(mgr.clone(), c).await?,
    xscore::cli::Commands::Config(c) => xscore::cli::config::run(c).await?,
}
```

**Step 4: Smoke**

```bash
cargo run -p xscore -- config path
cargo run -p xscore -- config show
cargo run -p xscore -- config set tmux_socket mysock
cargo run -p xscore -- config show
cargo run -p xscore -- config unset tmux_socket
```

**Step 5: Commit**

```bash
git add crates/xscore/src/cli/config.rs crates/xscore/src/model/config.rs crates/xscore/src/main.rs
git commit -m "feat(cli): config show/path/set/unset over AppConfig JSON"
```

---

#### Task 15: Integration tests + README

**Objective:** Real binary tests + URL-driven README rewrite.

**Files:**
- Create: `crates/xscore/tests/cli_smoke.rs`
- Create: `crates/xscore/tests/url_parse.rs`
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
    assert!(s.contains("session"));
    assert!(s.contains("data"));
    assert!(s.contains("config"));
}

#[test]
fn xscore_connect_shell_cat_and_close() {
    let create = Command::new(env!("CARGO_BIN_EXE_xscore"))
        .args(["connect", "shell:///bin/cat"]).output().unwrap();
    assert!(create.status.success(), "create stderr: {}", String::from_utf8_lossy(&create.stderr));
    let id = String::from_utf8(create.stdout).unwrap()
        .trim().strip_prefix("created: ").unwrap().to_string();

    let list = Command::new(env!("CARGO_BIN_EXE_xscore"))
        .args(["session", "list"]).output().unwrap();
    assert!(String::from_utf8_lossy(&list.stdout).contains(&id));

    let close = Command::new(env!("CARGO_BIN_EXE_xscore"))
        .args(["session", "close", "--id", &id]).output().unwrap();
    assert!(close.status.success());
}

#[test]
fn xscore_config_path_prints_paths() {
    let out = Command::new(env!("CARGO_BIN_EXE_xscore"))
        .args(["config", "path"]).output().unwrap();
    assert!(out.status.success());
    let s = String::from_utf8(out.stdout).unwrap();
    assert!(s.contains("xscore"));
}
```

**Step 2: URL parser test (unit, in url/connect.rs — already covered by Task 3)**

**Step 3: README**

```markdown
# xscore

Rust session manager for SSH / shell / serial. Sessions are opened from URLs and routed
through a `Transport × Sink` composition so the same CLI works for local tmux, remote
tmux-in-ssh, and plain shells.

## Quick start

\`\`\`
xscore connect shell:///bin/bash
xscore connect ssh://user@host:22?tmux=on
xscore connect serial:///dev/ttyUSB0?baud=115200
\`\`\`

## Commands

\`\`\`
xscore connect <url> [--save]
xscore session list
xscore session show --id <uuid>
xscore session close --id <uuid>
xscore session save --id <uuid>
xscore session load --path <file>

xscore data read --id <uuid> [--bytes N]
xscore data write --id <uuid> [--from-file FILE]
xscore data resize --id <uuid> --cols C --rows R

xscore config show
xscore config path
xscore config set <key> <value>
xscore config unset <key>
\`\`\`

## Architecture

\`\`\`
CLI  →  SessionManager  →  CompositeBackend(Transport, Sink)
                               │
            ┌──────────────────┼──────────────────┐
        Transport             Sink              Result
     shell/serial/ssh     tmux/raw            bytes
            │                  │
            ▼                  ▼
     local / ssh channel   external tmux server
                           on -L xscore socket
\`\`\`

Every session pairs one Transport with one Sink. The same `TmuxSink` works on shell (local tmux),
ssh (remote tmux running inside the channel), and serial (passthrough wrapper). A future
`RawSink` (or `tmux=off`) supports passthrough.
```

**Step 4: Run all tests**

Run: `cargo test -p xscore`
Expected: all unit + integration tests pass.

**Step 5: Commit**

```bash
git add tests/ README.md crates/xscore/src/cli/
git commit -m "test+docs: URL-driven CLI smoke tests + README rewrite"
```

---

## Acceptance Criteria

- `cargo build -p xscore` → success
- `cargo test -p xscore` → all tests pass (unit + integration)
- `xscore --help` lists `connect`, `session`, `data`, `config`
- `xscore connect shell:///bin/bash` creates a session + tmux pane on `-L xscore`
- `xscore connect ssh://...` works in URL form (SshTransport still deferred but URL parsing passes)
- `xscore connect serial:///dev/ttyUSB0?baud=115200` parses and validates
- `xscore session list/show/close/save/load` all function over `ConnectSpec`
- `xscore data read/write/resize` round-trips through `CompositeBackend`
- `xscore config show/path/set/unset` operates on `~/.config/xscore/config.json`
- Transport × Sink composition is observable: the same TmuxSink works on shell/serial/ssh transports without per-case glue code in `SessionManager` — the factory decides.

## What's deferred (explicitly NOT in this slice)

- Real russh connect + pty channel (Task 9 skeleton; SSH-integration phase)
- Real serial port open (Task 8 skeleton; serial-integration phase)
- TmuxSink `attach()` for "remote" mode (currently a no-op; will wire when SshTransport gets real IO)
- Live streaming subscribe (`Sink::snapshot` only for now)
- TmuxSink per-char send-keys perf optimization (one char = one tmux spawn; fix in next PR)
- Cross-process manager state (CLI = in-memory; MCP server = daemon later)
- MCP server, IPC server, web UI — land in subsequent slices once this URL surface stabilizes
- RawSink full implementation (currently a passthrough stub — `?tmux=off` works at parse time but the sink does nothing useful; tracked in open question below)

## Open questions (will be revisited, do not block the slice)

1. **Serial + TmuxSink:** tmux expects a PTY. The slice resolves this by running tmux locally and bridging serial bytes through tmux's pane input/output. A "real" tmux-attach-to-serial design is deferred.
2. **?tmux=off semantics:** `tmux=off` produces a `RawSink` that today is a no-op for capture/write. Future PR will wire it to a passthrough mode (raw transport I/O exposed directly).
3. **TmuxSink "auto" mode:** Resolves to `local` for shell/serial, `remote` for ssh. Future: an explicit `tmux_socket` query param to override.

## Execution Handoff

This plan is saved to `doc/dev/roadmap/cli-first-implementation-plan.md`.

Next step: execute via `subagent-driven-development` skill — fresh subagent per task, two-stage review (spec compliance then code quality). Each task ends with `cargo test -p xscore` + `cargo build -p xscore` before commit.

Want me to proceed with execution, or revise the slice first?