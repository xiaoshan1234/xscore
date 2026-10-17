# Task 6: Link + Pane + Registries + SessionManager

**Objective:** LinkRegistry (transport-address routing inside the manager), PaneRegistry, BackendRegistry, SessionManager that knows about all three. ADR 0005 retired `--link=existing`; `LinkRegistry.find_by_address` survives only as an internal routing helper.

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

    /// Add a new link: always creates a brand-new external resource.
    /// Returns (link_id, pane_id). The first pane is created automatically.
    ///
    /// At least one of `<url>` / `-c CONFIG` must be supplied (validated in
    /// the CLI layer). The address derived from the URL (or template) is
    /// used for routing inside `LinkRegistry` only — it is no longer a
    /// user-facing dedup key (ADR 0005 retired `--link=existing`).
    ///
    /// Note: this is renamed from `connect` to align with `xscore link add`.
    /// The full implementation lives in `task-10-cli-connect-link-pane.md`.
    pub async fn add_link(
        &self,
        spec: ConnectSpec,
        address: LinkAddress,
    ) -> Result<(LinkId, PaneId)> {
        // Resolve the link: always create a brand new link.
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
        let arc_link = self.links.insert(link).await;

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

    /// Attach to an existing tmux session (ADR 0005).
    /// Non-tmux transports return `Error::OpenOnlyForTmux(<kind>)`.
    /// For tmux: lookup the session by name on the appropriate server; if
    /// found, register a `LinkInfo` for it without creating a new tmux
    /// session; if not found, return `Error::LinkNotFound`.
    ///
    /// For this slice: stub. Real wiring lands when the SSH-integration
    /// phase resolves the remote tmux case; the local-tmux case is the
    /// only one wired up before that.
    pub async fn open_link(
        &self,
        spec: ConnectSpec,
        address: LinkAddress,
    ) -> Result<(LinkId, PaneId)> {
        match address.kind() {
            // tmux-only invariant (ADR 0005): non-tmux `open` is rejected.
            "shell" | "serial" | "ssh" => {
                return Err(Error::OpenOnlyForTmux(address.kind()));
            }
            "tmux" => {
                // Local tmux case: look up the session name in the saved tmux server
                // identified by spec.sink.tmux_socket; if found, register the link
                // without creating the session; if not found, LinkNotFound.
                // For this slice the implementation is stubbed — see
                // task-10 Step 6 for the open_link body that task-10 lands.
                todo!("stub for slice; real impl lands in tmux-attach persistence PR")
            }
            _ => unreachable!(),
        }
    }

    pub async fn backend(&self, id: PaneId) -> Result<Arc<dyn PaneBackend>> {
        self.backends.get(id).await.ok_or(Error::PaneNotFound(id))
    }

    /// Detach a tmux session from the in-memory registry without killing
    /// the external resource (ADR 0005: `xscore link close` is tmux-only
    /// and tmux session keeps running). Non-tmux transports return
    /// `Error::CloseOnlyForTmux(<kind>)`.
    pub async fn close_link(&self, id: LinkId) -> Result<()> {
        let arc_link = self.links.get(id).await.ok_or(Error::LinkNotFound(id))?;
        let kind = arc_link.info.address.kind();
        // tmux-only invariant (ADR 0005)
        if kind != "tmux" {
            return Err(Error::CloseOnlyForTmux(kind));
        }
        // Detach semantics: remove panes from in-memory registries only.
        // Do NOT call sink.close() / transport.close() — the tmux session
        // (and its server) keep running on the external side.
        let panes = arc_link.panes.lock().await.clone();
        for pid in panes {
            self.backends.remove(pid).await;
            self.panes.remove(pid).await;
        }
        self.links.remove(id).await;
        Ok(())
    }

    /// Full teardown of a link: kill transport + sink + every pane, and
    /// for tmux, kill the per-link tmux server. Was previously named
    /// `close_link`; renamed to `del_link` to match the new CLI verb
    /// (ADR 0005: `xscore link del`).
    pub async fn del_link(&self, id: LinkId) -> Result<()> {
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

    /// Full teardown of a pane: kill the tmux window + remove the backend
    /// + unregister from in-memory. Was previously named `close_pane`;
    /// renamed to `del_pane` to match the CLI verb (mirrors the link
    /// tree's add/del shape). Cross-invocation re-attach goes through
    /// `xscore link open`, not through panes.
    pub async fn del_pane(&self, id: PaneId) -> Result<()> {
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
git commit -m "feat(core): Link + Pane + Registries + SessionManager (link add/open/close/del, pane add/del)"
```