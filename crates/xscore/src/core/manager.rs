// core/manager.rs
//
// SessionManager — v0.3.0 (ADR 0007 v2, 3-entity decoupled model).
//
// All methods are async; all errors flow through `core::Error`. The manager
// holds `Arc<...>` registries so cloning the manager is cheap and registry
// references can be shared into background tasks.
//
// The 3 entities (Link, Pane, Window) are created/owned INDEPENDENTLY:
//   - `add_link(spec, addr, id)` → ONLY creates the link. NO window, NO pane.
//   - `add_window(name, layout, cols, rows)` → ONLY creates a window.
//   - `add_pane(name, cols, rows, x, y)` → ONLY creates a pane (orphan).
// Associations are dynamic:
//   - `attach_pane(pid, lid)` binds a pane to a link (creates the bridge).
//     For direct (raw) links this enforces Q1 (one active pane at a time —
//     the previously-active pane is auto-detached).
//   - `detach_pane(pid)` releases the link (closes the bridge; pane keeps
//     its buffer content).
//   - `add_pane_to_window(wid, pid)` puts a pane in a window (UI parent).
//   - `remove_pane_from_window(wid, pid)` removes a pane from a window
//     (pane stays alive as orphan).
//   - `resize_pane(pid, cols, rows)` updates the pane's geometry AND, if
//     attached, the bridge's resize.
//   - `set_window_layout(wid, layout)` updates the window's layout.
//
// Data ops (`data_read` / `data_write` / `data_resize`) operate on the
// pane's bridge. For orphan panes they return `Error::NotAttached(pid)`.
//
// State persistence (save_state / load_from_state) walks the three flat
// registries and serializes a `StateSnapshot` (see `core::state`). The
// v0.3.0 schema is NOT backward-compatible with v0.2.0 — users must
// clear `state.json` on upgrade (documented in CHANGELOG).

use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;

use super::{Error, LinkAddress, Result};
use super::id::{LinkId, PaneId, WindowId};
use super::link::Link;
use super::link_registry::LinkRegistry;
use super::pane_registry::PaneRegistry;
use super::window::Window;
use super::window_registry::WindowRegistry;

use crate::backends::{DirectBridge, PaneBridge, TmuxBridge};
use crate::model::{
    ConnectSpec, PaneInfo, SinkConfig, WindowInfo, WindowLayout,
};
use crate::sink::factory::sink_for;
use crate::sink::tmux::TmuxSink;
use crate::sink::Sink;
use crate::transport::factory as transport_factory;
use crate::transport::NoopTransport;
use crate::util::tmux_arg_socket;

#[derive(Clone)]
pub struct SessionManager {
    pub links: Arc<LinkRegistry>,
    pub windows: Arc<WindowRegistry>,
    pub panes: Arc<PaneRegistry>,
    /// v0.3.0: per-pane bridge map. Stored SEPARATELY from `PaneInfo`
    /// so mutating the info (which is JSON-serializable) doesn't destroy
    /// the bridge (which is not). Bridges are in-memory only — they are
    /// rebuilt on `attach_pane` after process restart.
    pane_bridges: Arc<Mutex<HashMap<PaneId, Arc<dyn PaneBridge>>>>,
}

impl Default for SessionManager {
    fn default() -> Self { Self::new() }
}

impl SessionManager {
    pub fn new() -> Self {
        Self {
            links: Arc::new(LinkRegistry::new()),
            windows: Arc::new(WindowRegistry::new()),
            panes: Arc::new(PaneRegistry::new()),
            pane_bridges: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    // ===== add_link (lean — no auto window/pane in v0.3.0) =====

    /// Create a new link. Returns just the `LinkId`.
    ///
    /// **v0.3.0:** this does NOT auto-create a window or pane. To get
    /// data I/O you must explicitly create a pane and attach it via
    /// `attach_pane`. (For tmux-backed links, attach also creates the
    /// tmux split-window; for raw links, attach wraps the source for
    /// `data_read`/`data_write`.)
    pub async fn add_link(
        &self,
        spec: ConnectSpec,
        address: LinkAddress,
        id: Option<LinkId>,
    ) -> Result<LinkId> {
        let link_id = match id {
            Some(supplied) if self.links.get(supplied).await.is_some() => {
                return Err(Error::LinkIdExists(supplied));
            }
            Some(supplied) => supplied,
            None => LinkId::new(),
        };

        let transport = transport_factory::transport_for(spec.transport.clone()).await?;
        let transport_kind: &'static str = transport.kind();
        let mut transport_arc: Arc<dyn crate::transport::Transport> =
            Arc::from(transport);

        let transport_mut: &mut dyn crate::transport::Transport =
            Arc::get_mut(&mut transport_arc)
                .ok_or_else(|| Error::Config(
                    "transport Arc unexpectedly shared at link-construction time".into()
                ))?;
        let sink = sink_for(link_id, &spec.sink, transport_kind, transport_mut).await?;

        let tmux_sink = match &sink {
            raw if raw.kind() == "raw" => None,
            tmux if tmux.kind() == "tmux" => {
                let (mode, session_name) = sink_info_from_spec(&spec, link_id);
                Some(Arc::new(TmuxSink::adopt(link_id, &mode, &session_name)))
            }
            _ => None,
        };

        let link_info = crate::model::LinkInfo {
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
            transport: transport_arc,
            sink,
            tmux_sink,
            active_pane: Mutex::new(None),
        };
        self.links.insert(link).await;

        self.save_state().await?;
        Ok(link_id)
    }

    /// Attach to an existing tmux session (ADR 0005). Stub for the tmux
    /// case — full wiring lands in the tmux-attach persistence PR.
    pub async fn open_link(
        &self,
        _spec: ConnectSpec,
        address: LinkAddress,
    ) -> Result<LinkId> {
        let kind = address.kind();
        if kind != "tmux" {
            return Err(Error::OpenOnlyForTmux(kind));
        }
        todo!("stub for slice; real impl lands in tmux-attach persistence PR")
    }

    /// Full teardown of a link: close the bridge on every attached pane
    /// (silently; panes survive as orphans), then close the sink + transport.
    pub async fn del_link(&self, id: LinkId) -> Result<()> {
        let arc_link = self.links.get(id).await.ok_or(Error::LinkNotFound(id))?;
        // Detach every pane whose info.link_id == Some(id).
        let mut to_clear: Vec<(PaneId, Option<Arc<dyn PaneBridge>>)> = Vec::new();
        for info in self.panes.list_all().await {
            if info.link_id == Some(id) {
                let bridge = self.pane_bridges.lock().await.remove(&info.id);
                to_clear.push((info.id, bridge));
            }
        }
        for (pid, bridge) in to_clear {
            if let Some(b) = bridge {
                let _ = b.close().await;
            }
            if let Some(mut info) = self.panes.get(pid).await {
                info.link_id = None;
                self.panes.insert(info).await;
            }
        }
        arc_link.sink.close().await?;
        arc_link.transport.close().await?;
        self.links.remove(id).await;
        self.save_state().await?;
        Ok(())
    }

    pub async fn close_link(&self, id: LinkId) -> Result<()> {
        let arc_link = self.links.get(id).await.ok_or(Error::LinkNotFound(id))?;
        let kind = arc_link.info.transport_str();
        if kind != "tmux" {
            return Err(Error::CloseOnlyForTmux(kind));
        }
        Ok(())
    }

    // ===== add_window / del_window / set_window_layout =====

    pub async fn add_window(
        &self,
        name: Option<String>,
        layout: WindowLayout,
        cols: u16,
        rows: u16,
    ) -> Result<WindowId> {
        let window_id = WindowId::new();
        let resolved_name = name.unwrap_or_else(|| format!("w-{}", &window_id.to_string()[..8]));
        let info = WindowInfo {
            id: window_id,
            name: resolved_name,
            created_at: chrono::Utc::now(),
            layout,
            cols,
            rows,
        };
        let window = Window::new(info);
        self.windows.insert(window).await;
        self.save_state().await?;
        Ok(window_id)
    }

    pub async fn del_window(&self, id: WindowId) -> Result<()> {
        let arc_window = self.windows.get(id).await.ok_or(Error::WindowNotFound(id))?;
        let pids: Vec<PaneId> = arc_window.panes.lock().await.clone();
        for pid in pids {
            if let Some(mut info) = self.panes.get(pid).await {
                info.window_id = None;
                self.panes.insert(info).await;
            }
        }
        self.windows.remove(id).await;
        self.save_state().await?;
        Ok(())
    }

    pub async fn set_window_layout(&self, id: WindowId, layout: WindowLayout) -> Result<()> {
        let old = self.windows.get(id).await.ok_or(Error::WindowNotFound(id))?;
        let mut info = old.info.clone();
        info.layout = layout;
        let updated = Window::new(info);
        let old_panes = old.panes.lock().await.clone();
        *updated.panes.lock().await = old_panes;
        self.windows.insert(updated).await;
        self.save_state().await?;
        Ok(())
    }

    // ===== add_pane / del_pane =====

    pub async fn add_pane(
        &self,
        name: Option<String>,
        cols: u16,
        rows: u16,
        x: u16,
        y: u16,
    ) -> Result<PaneId> {
        let pane_id = PaneId::new();
        let resolved_name = name.unwrap_or_else(|| format!("p-{pane_id}"));
        let info = PaneInfo {
            id: pane_id,
            name: resolved_name,
            created_at: chrono::Utc::now(),
            cols,
            rows,
            x,
            y,
            link_id: None,
            window_id: None,
        };
        self.panes.insert(info).await;
        self.save_state().await?;
        Ok(pane_id)
    }

    pub async fn del_pane(&self, id: PaneId) -> Result<()> {
        let info = self.panes.get(id).await.ok_or(Error::PaneNotFound(id))?;
        // Drop bridge if attached.
        let bridge = self.pane_bridges.lock().await.remove(&id);
        if let Some(b) = bridge {
            let _ = b.close().await;
        }
        if let Some(lid) = info.link_id {
            if let Some(arc_link) = self.links.get(lid).await {
                let mut active = arc_link.active_pane.lock().await;
                if *active == Some(id) {
                    *active = None;
                }
            }
        }
        if let Some(wid) = info.window_id {
            if let Some(arc_window) = self.windows.get(wid).await {
                arc_window.panes.lock().await.retain(|x| *x != id);
            }
        }
        self.panes.remove(id).await;
        self.save_state().await?;
        Ok(())
    }

    // ===== attach_pane / detach_pane (Q1 logic) =====

    pub async fn attach_pane(&self, pid: PaneId, lid: LinkId) -> Result<()> {
        let arc_link = self.links.get(lid).await.ok_or(Error::LinkNotFound(lid))?;
        // Confirm pane exists (no Arc<Pane> needed — just check info).
        let _info = self.panes.get(pid).await.ok_or(Error::PaneNotFound(pid))?;

        // Q1: for direct (raw) links, auto-detach any OTHER pane whose
        // info.link_id == Some(lid). Scan pane info (the source of
        // truth, persisted in state.json).
        if matches!(arc_link.info.sink, SinkConfig::Raw) {
            let mut to_detach: Vec<PaneId> = Vec::new();
            for p in self.panes.list_all().await {
                if p.id != pid && p.link_id == Some(lid) {
                    to_detach.push(p.id);
                }
            }
            for old_pid in to_detach {
                let old_bridge = self.pane_bridges.lock().await.remove(&old_pid);
                if let Some(b) = old_bridge {
                    let _ = b.close().await;
                }
                if let Some(mut old_info) = self.panes.get(old_pid).await {
                    old_info.link_id = None;
                    self.panes.insert(old_info).await;
                }
                tracing::info!(
                    "Q1: auto-detached pane {old_pid} from link {lid} (new attach: {pid})"
                );
            }
            let mut active = arc_link.active_pane.lock().await;
            *active = Some(pid);
        }

        // Build the bridge.
        let bridge: Arc<dyn PaneBridge> = match &arc_link.info.sink {
            SinkConfig::Raw => Arc::new(DirectBridge {
                transport: Arc::clone(&arc_link.transport),
            }),
            SinkConfig::Tmux { .. } => {
                let tmux_arc = arc_link.tmux_sink.as_ref()
                    .ok_or_else(|| Error::Sink("tmux-backed link has no TmuxSink".into()))?
                    .clone();
                let handle = tmux_arc.new_pane().await?;
                Arc::new(TmuxBridge {
                    sink: Arc::downgrade(&tmux_arc),
                    handle,
                })
            }
        };

        // Store bridge in the dedicated map (NOT in pane info — bridges
        // are runtime-only and reconstructing them via Pane::new would
        // destroy the bridge).
        self.pane_bridges.lock().await.insert(pid, bridge);

        // Update pane info.link_id.
        if let Some(mut info) = self.panes.get(pid).await {
            info.link_id = Some(lid);
            self.panes.insert(info).await;
        }

        self.save_state().await?;
        Ok(())
    }

    pub async fn detach_pane(&self, pid: PaneId) -> Result<()> {
        let info = self.panes.get(pid).await.ok_or(Error::PaneNotFound(pid))?;
        let lid = match info.link_id {
            Some(l) => l,
            None => return Err(Error::NotAttached(pid)),
        };
        let bridge = self.pane_bridges.lock().await.remove(&pid);
        if let Some(b) = bridge {
            let _ = b.close().await;
        }
        if let Some(arc_link) = self.links.get(lid).await {
            let mut active = arc_link.active_pane.lock().await;
            if *active == Some(pid) {
                *active = None;
            }
        }
        if let Some(mut info) = self.panes.get(pid).await {
            info.link_id = None;
            self.panes.insert(info).await;
        }
        self.save_state().await?;
        Ok(())
    }

    // ===== window ⇄ pane association =====

    pub async fn add_pane_to_window(&self, wid: WindowId, pid: PaneId) -> Result<()> {
        let arc_window = self.windows.get(wid).await.ok_or(Error::WindowNotFound(wid))?;
        let _ = self.panes.get(pid).await.ok_or(Error::PaneNotFound(pid))?;
        {
            let panes = arc_window.panes.lock().await;
            if panes.contains(&pid) {
                return Err(Error::AlreadyInWindow { window: wid, pane: pid });
            }
        }
        if let Some(mut info) = self.panes.get(pid).await {
            info.window_id = Some(wid);
            self.panes.insert(info).await;
        }
        arc_window.panes.lock().await.push(pid);
        self.save_state().await?;
        Ok(())
    }

    pub async fn remove_pane_from_window(&self, wid: WindowId, pid: PaneId) -> Result<()> {
        let arc_window = self.windows.get(wid).await.ok_or(Error::WindowNotFound(wid))?;
        let _ = self.panes.get(pid).await.ok_or(Error::PaneNotFound(pid))?;
        {
            let mut panes = arc_window.panes.lock().await;
            if !panes.contains(&pid) {
                return Err(Error::NotInWindow { window: wid, pane: pid });
            }
            panes.retain(|p| *p != pid);
        }
        if let Some(mut info) = self.panes.get(pid).await {
            info.window_id = None;
            self.panes.insert(info).await;
        }
        self.save_state().await?;
        Ok(())
    }

    pub async fn move_pane_to_window(&self, pid: PaneId, wid: WindowId) -> Result<()> {
        let info = self.panes.get(pid).await.ok_or(Error::PaneNotFound(pid))?;
        let current_wid = info.window_id;
        if let Some(old_wid) = current_wid {
            if old_wid != wid {
                self.remove_pane_from_window(old_wid, pid).await?;
            }
        }
        self.add_pane_to_window(wid, pid).await
    }

    // ===== geometry updates =====

    pub async fn resize_pane(&self, pid: PaneId, cols: u16, rows: u16) -> Result<()> {
        let _ = self.panes.get(pid).await.ok_or(Error::PaneNotFound(pid))?;
        if let Some(mut info) = self.panes.get(pid).await {
            info.cols = cols;
            info.rows = rows;
            self.panes.insert(info).await;
        }
        let bridge = self.pane_bridges.lock().await.get(&pid).cloned();
        if let Some(b) = bridge {
            b.resize(cols, rows).await?;
        }
        self.save_state().await?;
        Ok(())
    }

    pub async fn move_pane(&self, pid: PaneId, x: u16, y: u16) -> Result<()> {
        let _ = self.panes.get(pid).await.ok_or(Error::PaneNotFound(pid))?;
        if let Some(mut info) = self.panes.get(pid).await {
            info.x = x;
            info.y = y;
            self.panes.insert(info).await;
        }
        self.save_state().await?;
        Ok(())
    }

    // ===== data ops (read / write / resize via bridge) =====

    pub async fn data_read(&self, pid: PaneId, buf: &mut [u8]) -> Result<usize> {
        let _ = self.panes.get(pid).await.ok_or(Error::PaneNotFound(pid))?;
        let bridge = self.pane_bridges.lock().await.get(&pid).cloned();
        match bridge {
            Some(b) => b.read(buf).await,
            None => Err(Error::NotAttached(pid)),
        }
    }

    pub async fn data_write(&self, pid: PaneId, data: &[u8]) -> Result<usize> {
        let _ = self.panes.get(pid).await.ok_or(Error::PaneNotFound(pid))?;
        let bridge = self.pane_bridges.lock().await.get(&pid).cloned();
        match bridge {
            Some(b) => b.write(data).await,
            None => Err(Error::NotAttached(pid)),
        }
    }

    /// Resize a pane's bridge AND its info (cols/rows). Errors with
    /// `NotAttached(pid)` for orphan panes — use `resize_pane` if you
    /// want to update geometry without going through the bridge.
    pub async fn data_resize(&self, pid: PaneId, cols: u16, rows: u16) -> Result<()> {
        let _ = self.panes.get(pid).await.ok_or(Error::PaneNotFound(pid))?;
        let bridge = self.pane_bridges.lock().await.get(&pid).cloned();
        match bridge {
            Some(b) => {
                b.resize(cols, rows).await?;
                if let Some(mut info) = self.panes.get(pid).await {
                    info.cols = cols;
                    info.rows = rows;
                    self.panes.insert(info).await;
                }
                self.save_state().await?;
                Ok(())
            }
            None => Err(Error::NotAttached(pid)),
        }
    }

    // ===== state persistence =====

    pub async fn save_state(&self) -> Result<()> {
        let arc_links = self.links.list().await;
        let links: Vec<crate::model::LinkInfo> = arc_links.iter().map(|l| l.info.clone()).collect();

        let arc_windows = self.windows.list_all().await;
        let mut windows: Vec<super::state::WindowSnapshot> = Vec::new();
        for w in &arc_windows {
            let pane_ids = w.panes.lock().await.clone();
            windows.push(super::state::WindowSnapshot {
                info: w.info.clone(),
                panes: pane_ids,
            });
        }

        let panes = self.panes.list_all().await;

        let snapshot = super::state::StateSnapshot {
            links,
            windows,
            panes,
        };
        tokio::task::spawn_blocking(move || super::state::save(&snapshot))
            .await
            .map_err(|e| Error::Config(format!("join: {e}")))?
    }

    pub async fn load_from_state() -> Result<Self> {
        match super::state::load()? {
            Some(s) => Self::hydrate(s).await,
            None => Ok(Self::new()),
        }
    }

    async fn hydrate(snapshot: super::state::StateSnapshot) -> Result<Self> {
        let mgr = Self::new();
        for info in snapshot.links {
            if let Err(e) = mgr.hydrate_link(info).await {
                tracing::warn!("hydration: skipping link: {e}");
            }
        }
        for ws in snapshot.windows {
            let w = Window::new(ws.info);
            for pid in ws.panes {
                w.panes.lock().await.push(pid);
            }
            mgr.windows.insert(w).await;
        }
        for pinfo in snapshot.panes {
            mgr.panes.insert(pinfo).await;
        }
        Ok(mgr)
    }

    async fn hydrate_link(&self, info: crate::model::LinkInfo) -> Result<()> {
        let link_id = info.id;
        let session_name = format!("xscore-{link_id}");
        let transport: Arc<dyn crate::transport::Transport> = Arc::new(NoopTransport);
        let sink: Box<dyn crate::sink::Sink> = match &info.sink {
            SinkConfig::Raw => Box::new(crate::sink::raw::RawSink),
            SinkConfig::Tmux { mode, .. } => {
                let has = tokio::process::Command::new("tmux")
                    .args(tmux_arg_socket())
                    .args(["has-session", "-t", &session_name])
                    .status().await?;
                if !has.success() {
                    return Err(Error::Sink(format!(
                        "tmux session {session_name} gone, skipping link {link_id}"
                    )));
                }
                Box::new(TmuxSink::adopt(link_id, mode, &session_name))
            }
        };
        let tmux_sink = match &info.sink {
            SinkConfig::Raw => None,
            SinkConfig::Tmux { mode, .. } => {
                Some(Arc::new(TmuxSink::adopt(link_id, mode, &session_name)))
            }
        };
        let link = Link {
            id: link_id,
            info,
            transport,
            sink,
            tmux_sink,
            active_pane: Mutex::new(None),
        };
        self.links.insert(link).await;
        Ok(())
    }
}

fn sink_info_from_spec(spec: &ConnectSpec, link_id: LinkId) -> (String, String) {
    match &spec.sink {
        SinkConfig::Raw => ("raw".into(), String::new()),
        SinkConfig::Tmux { mode, session_name } => {
            let name = session_name.clone()
                .unwrap_or_else(|| format!("xscore-{link_id}"));
            (mode.clone(), name)
        }
    }
}