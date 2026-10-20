// core/manager.rs (v0.4.0 — tmux adoption, ADR 0008)
//
// All methods are async; all errors flow through `core::Error`. The
// manager holds `Arc<...>` registries so cloning the manager is cheap.
//
// Creation semantics changed in v0.4.0:
//   - `add_link(spec, addr, id, pane)` ALWAYS yields a usable pane.
//       * tmux link        → adopts the tmux session's first window and
//                            first pane (tmux is the source of truth for
//                            `@N`/`%N` and for geometry).
//       * direct + `pane`  → binds that existing (hollow) pane.
//       * direct + no pane → creates a virtual window + a pane and its
//                            `DirectBridge` (ring buffer).
//   - `add_window(..., link)` creates a tmux window when a tmux link is
//     given (tmux reports the handle/geometry), else a virtual window.
//   - `add_pane(..., window)` splits inside a real tmux window when the
//     window has a `tmux_window_handle`; otherwise it is a hollow pane.
//
// `detach_pane` never touches a shared transport (P0-a) and refuses for
// tmux-owned panes (they are tmux's, not ours). `del_link` on a tmux
// link kills the session and drops the panes with it; on a direct link
// it orphans the panes and keeps their buffers.
//
// State persistence: `save_state` writes `StateSnapshot` (schema v2);
// see `core::state` for the version gate.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use tokio::sync::Mutex;

use super::{Error, LinkAddress, Result};
use super::id::{LinkId, PaneId, WindowId};
use super::link::Link;
use super::link_registry::LinkRegistry;
use super::pane_registry::PaneRegistry;
use super::window::Window;
use super::window_registry::WindowRegistry;

use crate::backends::{DirectBridge, PaneBridge, RingBuffer, TmuxBridge};
use crate::model::{
    ConnectSpec, PaneInfo, SinkConfig, WindowInfo, WindowLayout,
};
use crate::sink::factory::sink_for;
use crate::sink::tmux::TmuxSink;
use crate::sink::{layout_from_tmux, Sink};
use crate::transport::factory as transport_factory;
use crate::transport::NoopTransport;
use crate::util::tmux_arg_socket;

/// What `link add` produced. v0.4.0: a link always comes with a pane
/// (and usually a window), so the CLI can print all three ids.
#[derive(Debug, Clone, Copy)]
pub struct LinkAddResult {
    pub link_id: LinkId,
    pub window_id: Option<WindowId>,
    pub pane_id: PaneId,
}

/// Drift report for `link sync` (v0.4.0). Handles are `@N`/`%N`, never
/// indexes.
#[derive(Debug, Clone, Default)]
pub struct SyncReport {
    /// Windows tmux has but xscore does not track.
    pub missing_windows: Vec<String>,
    /// Windows xscore tracks but tmux no longer has.
    pub stale_windows: Vec<String>,
    /// Panes tmux has in a tracked window but xscore does not track
    /// (never adopted — report only).
    pub missing_panes: Vec<String>,
    /// Panes xscore tracks but tmux no longer has.
    pub stale_panes: Vec<String>,
    /// Registry rows corrected in place (geometry etc.).
    pub healed: usize,
    /// Registry rows dropped by `--heal`.
    pub removed: usize,
}

impl SyncReport {
    pub fn is_clean(&self) -> bool {
        self.missing_windows.is_empty()
            && self.stale_windows.is_empty()
            && self.missing_panes.is_empty()
            && self.stale_panes.is_empty()
    }
}

#[derive(Clone)]
pub struct SessionManager {
    pub links: Arc<LinkRegistry>,
    pub windows: Arc<WindowRegistry>,
    pub panes: Arc<PaneRegistry>,
    /// Per-pane bridge map. Stored SEPARATELY from `PaneInfo` so mutating
    /// the info (which is JSON-serializable) doesn't destroy the bridge.
    /// Bridges are in-memory only.
    pane_bridges: Arc<Mutex<HashMap<PaneId, Arc<dyn PaneBridge>>>>,
    /// Per-pane direct-pane buffers. They OUTLIVE the bridge: detaching a
    /// direct pane drops the bridge but keeps the history (ADR 0008).
    pane_buffers: Arc<Mutex<HashMap<PaneId, Arc<Mutex<RingBuffer>>>>>,
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
            pane_buffers: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    // ===== add_link =====

    /// Create a link and give it a usable pane. See the module comment
    /// for the three branches.
    pub async fn add_link(
        &self,
        spec: ConnectSpec,
        address: LinkAddress,
        id: Option<LinkId>,
        pane: Option<PaneId>,
    ) -> Result<LinkAddResult> {
        // --- validate before allocating anything (a rejected call must
        //     not leave a tmux session or a spawned child behind) ---
        let is_tmux = !matches!(spec.sink, SinkConfig::Raw);
        if is_tmux && pane.is_some() {
            return Err(Error::TmuxLinkCannotBindPane);
        }
        if let Some(pid) = pane {
            let info = self.panes.get(pid).await.ok_or(Error::PaneNotFound(pid))?;
            if let Some(existing) = info.link_id {
                return Err(Error::PaneAlreadyAttached(pid, existing));
            }
            if info.sink_handle.is_some() {
                // tmux-owned panes are never re-bound: their identity is
                // the `%N` handle tmux gave them.
                return Err(Error::TmuxLinkCannotBindPane);
            }
        }

        let link_id = match id {
            Some(supplied) if self.links.get(supplied).await.is_some() => {
                return Err(Error::LinkIdExists(supplied));
            }
            Some(supplied) => supplied,
            None => LinkId::new(),
        };

        let transport = transport_factory::transport_for(spec.transport.clone()).await?;
        let transport_kind: &'static str = transport.kind();
        let mut transport_arc: Arc<dyn crate::transport::Transport> = Arc::from(transport);

        let transport_mut: &mut dyn crate::transport::Transport =
            Arc::get_mut(&mut transport_arc)
                .ok_or_else(|| Error::Config(
                    "transport Arc unexpectedly shared at link-construction time".into()
                ))?;
        let sink = sink_for(link_id, &spec.sink, transport_kind, transport_mut).await?;
        // Wire the sink to the transport (no-op for both current impls, but
        // this is the documented once-per-link-lifetime hook).
        sink.attach(&*transport_mut).await?;

        let tmux_arc: Option<Arc<TmuxSink>> = match &spec.sink {
            SinkConfig::Raw => None,
            SinkConfig::Tmux { .. } => {
                let (mode, session_name) = sink_info_from_spec(&spec, link_id);
                Some(Arc::new(TmuxSink::adopt(link_id, &mode, &session_name)))
            }
        };

        // --- branch: decide the window / pane / bridge ---
        type Branch = (
            Option<WindowId>,
            PaneId,
            PaneInfo,
            Option<Arc<dyn PaneBridge>>,
            Option<Window>,
        );
        let (window_id, pane_id, pane_record, bridge, window_record): Branch =
            match (&spec.sink, pane) {
            // (a) tmux link: adopt what tmux created for us.
            (SinkConfig::Tmux { .. }, _) => {
                let ts = tmux_arc.as_ref().expect("tmux sink present");
                let win = first_window_with_retry(ts).await?;
                let wid = WindowId::new();
                let w = Window::new(WindowInfo {
                    id: wid,
                    name: win.name.clone(),
                    created_at: chrono::Utc::now(),
                    layout: layout_from_tmux(&win.layout),
                    cols: win.cols,
                    rows: win.rows,
                    tmux_window_handle: Some(win.handle.clone()),
                });
                let panes = ts.list_panes(&win.handle).await?;
                let first = panes.into_iter().next().ok_or_else(|| {
                    Error::Sink(format!("tmux window {} has no panes", win.handle))
                })?;
                let pid = PaneId::new();
                let _ = w.add_pane(pid).await;
                (
                    Some(wid),
                    pid,
                    PaneInfo {
                        id: pid,
                        name: format!("p-{pid}"),
                        created_at: chrono::Utc::now(),
                        cols: first.cols,
                        rows: first.rows,
                        x: first.x,
                        y: first.y,
                        link_id: Some(link_id),
                        window_id: Some(wid),
                        sink_handle: Some(first.handle.clone()),
                    },
                    Some(Arc::new(TmuxBridge {
                        sink: Arc::downgrade(ts),
                        handle: first.handle,
                    }) as Arc<dyn PaneBridge>),
                    Some(w),
                )
            }

            // (b) direct link bound to an existing pane: no window is
            //     created — the pane keeps whatever window it had.
            (SinkConfig::Raw, Some(pid)) => {
                let buf = self.buffer_for(pid).await;
                let mut info = self.panes.get(pid).await.expect("checked above");
                info.link_id = Some(link_id);
                (
                    None,
                    pid,
                    info,
                    Some(Arc::new(DirectBridge::new_with_buffer(
                        Arc::clone(&transport_arc), buf,
                    )) as Arc<dyn PaneBridge>),
                    None,
                )
            }

            // (c) direct link with no pane: virtual window + first pane.
            (SinkConfig::Raw, None) => {
                let wid = WindowId::new();
                let pid = PaneId::new();
                let w = Window::new(WindowInfo {
                    id: wid,
                    name: format!("w-{}", &wid.to_string()[..8]),
                    created_at: chrono::Utc::now(),
                    layout: WindowLayout::Single,
                    cols: 120,
                    rows: 30,
                    tmux_window_handle: None,
                });
                let _ = w.add_pane(pid).await;
                let buf = self.buffer_for(pid).await;
                (
                    Some(wid),
                    pid,
                    PaneInfo {
                        id: pid,
                        name: format!("p-{pid}"),
                        created_at: chrono::Utc::now(),
                        cols: 80,
                        rows: 24,
                        x: 0,
                        y: 0,
                        link_id: Some(link_id),
                        window_id: Some(wid),
                        sink_handle: None,
                    },
                    Some(Arc::new(DirectBridge::new_with_buffer(
                        Arc::clone(&transport_arc), buf,
                    )) as Arc<dyn PaneBridge>),
                    Some(w),
                )
            }
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
            tmux_sink: tmux_arc,
            active_pane: Mutex::new(Some(pane_id)),
        };
        self.links.insert(link).await;
        if let Some(w) = window_record {
            self.windows.insert(w).await;
        }
        self.panes.insert(pane_record).await;
        if let Some(b) = bridge {
            self.pane_bridges.lock().await.insert(pane_id, b);
        }

        self.save_state().await?;
        Ok(LinkAddResult { link_id, window_id, pane_id })
    }

    /// Attach to an existing tmux session (ADR 0005). Stub — full wiring
    /// lands in the tmux-attach persistence PR.
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

    /// Full teardown of a link.
    ///
    /// * tmux link: kill the session; every pane/window that came from it
    ///   is removed from the registry (no per-pane `kill-pane` needed —
    ///   the session takes them all down).
    /// * direct link: orphan every attached pane (buffer kept), close the
    ///   transport ONCE, remove the link.
    pub async fn del_link(&self, id: LinkId) -> Result<()> {
        let arc_link = self.links.get(id).await.ok_or(Error::LinkNotFound(id))?;

        if let Some(ts) = arc_link.tmux_sink.clone() {
            let handles: HashSet<String> = ts
                .list_windows()
                .await
                .unwrap_or_default()
                .into_iter()
                .map(|w| w.handle)
                .collect();

            // Windows that belong to this session, plus panes attached to
            // the link (the two sets should coincide; use the union so a
            // drifted registry is still cleaned up).
            let doomed_windows: Vec<WindowId> = self
                .windows
                .list_all()
                .await
                .iter()
                .filter(|w| match &w.info.tmux_window_handle {
                    Some(h) => handles.contains(h),
                    None => false,
                })
                .map(|w| w.info.id)
                .collect();

            let doomed_panes: Vec<PaneId> = self
                .panes
                .list_all()
                .await
                .iter()
                .filter(|p| {
                    p.link_id == Some(id)
                        || p.window_id.map(|w| doomed_windows.contains(&w)).unwrap_or(false)
                })
                .map(|p| p.id)
                .collect();

            {
                let mut bridges = self.pane_bridges.lock().await;
                let mut buffers = self.pane_buffers.lock().await;
                for pid in &doomed_panes {
                    // No close(): kill-session is about to take them down.
                    bridges.remove(pid);
                    buffers.remove(pid);
                }
            }
            for pid in &doomed_panes {
                self.panes.remove(*pid).await;
            }
            for wid in &doomed_windows {
                self.windows.remove(*wid).await;
            }
            ts.close().await?;
        } else {
            let attached: Vec<PaneId> = self
                .panes
                .list_all()
                .await
                .iter()
                .filter(|p| p.link_id == Some(id))
                .map(|p| p.id)
                .collect();
            for pid in attached {
                let bridge = self.pane_bridges.lock().await.remove(&pid);
                if let Some(b) = bridge {
                    // DirectBridge::close() stops the pump only — the
                    // transport below is closed exactly once, on purpose.
                    let _ = b.close().await;
                }
                if let Some(mut info) = self.panes.get(pid).await {
                    info.link_id = None;
                    self.panes.insert(info).await;
                }
            }
            arc_link.transport.close().await?;
        }

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

    /// Create a window. With `link: Some(tmux_lid)` a real tmux window is
    /// created (and adopted); `layout`/`cols`/`rows` are then IGNORED in
    /// favour of what tmux reports. With `link: None` the window is
    /// virtual (purely xscore-side).
    pub async fn add_window(
        &self,
        name: Option<String>,
        layout: WindowLayout,
        cols: u16,
        rows: u16,
        link: Option<LinkId>,
    ) -> Result<WindowId> {
        let window_id = WindowId::new();
        let resolved_name = name.unwrap_or_else(|| format!("w-{}", &window_id.to_string()[..8]));

        let info = match link {
            None => WindowInfo {
                id: window_id,
                name: resolved_name,
                created_at: chrono::Utc::now(),
                layout,
                cols,
                rows,
                tmux_window_handle: None,
            },
            Some(lid) => {
                let arc_link = self.links.get(lid).await.ok_or(Error::LinkNotFound(lid))?;
                if matches!(arc_link.info.sink, SinkConfig::Raw) {
                    return Err(Error::NotTmuxLink(lid));
                }
                let ts = arc_link.tmux_sink.clone()
                    .ok_or_else(|| Error::Sink("tmux-backed link has no TmuxSink".into()))?;
                let snap = ts.new_window(&resolved_name).await?;
                WindowInfo {
                    id: window_id,
                    name: snap.name.clone(),
                    created_at: chrono::Utc::now(),
                    layout: layout_from_tmux(&snap.layout),
                    cols: snap.cols,
                    rows: snap.rows,
                    tmux_window_handle: Some(snap.handle),
                }
            }
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

    /// Create a pane.
    ///
    /// * `window: None` → hollow orphan pane (no link, no window).
    /// * `window: Some(wid)` where the window has a `tmux_window_handle`
    ///   → split inside that real tmux window; the new pane is owned by
    ///   tmux, gets that window's link, and uses tmux's geometry.
    /// * `window: Some(wid)` virtual → hollow pane inside the window.
    pub async fn add_pane(
        &self,
        name: Option<String>,
        cols: u16,
        rows: u16,
        x: u16,
        y: u16,
        window: Option<WindowId>,
    ) -> Result<PaneId> {
        let pane_id = PaneId::new();
        let resolved_name = name.unwrap_or_else(|| format!("p-{pane_id}"));

        let (info, bridge) = match window {
            None => (
                PaneInfo {
                    id: pane_id,
                    name: resolved_name,
                    created_at: chrono::Utc::now(),
                    cols,
                    rows,
                    x,
                    y,
                    link_id: None,
                    window_id: None,
                    sink_handle: None,
                },
                None,
            ),
            Some(wid) => {
                let arc_window = self.windows.get(wid).await.ok_or(Error::WindowNotFound(wid))?;
                match arc_window.info.tmux_window_handle.clone() {
                    Some(handle) => {
                        let lid = self.link_for_window_handle(&handle).await?;
                        let ts = self.tmux_sink(lid).await?;
                        let snap = ts.split_pane(&handle).await?;
                        let info = PaneInfo {
                            id: pane_id,
                            name: resolved_name,
                            created_at: chrono::Utc::now(),
                            cols: snap.cols,
                            rows: snap.rows,
                            x: snap.x,
                            y: snap.y,
                            link_id: Some(lid),
                            window_id: Some(wid),
                            sink_handle: Some(snap.handle.clone()),
                        };
                        let bridge = Arc::new(TmuxBridge {
                            sink: Arc::downgrade(&ts),
                            handle: snap.handle,
                        }) as Arc<dyn PaneBridge>;
                        (info, Some(bridge))
                    }
                    None => (
                        PaneInfo {
                            id: pane_id,
                            name: resolved_name,
                            created_at: chrono::Utc::now(),
                            cols,
                            rows,
                            x,
                            y,
                            link_id: None,
                            window_id: Some(wid),
                            sink_handle: None,
                        },
                        None,
                    ),
                }
            }
        };

        self.panes.insert(info).await;
        if let Some(wid) = window {
            if let Some(arc_window) = self.windows.get(wid).await {
                arc_window.panes.lock().await.push(pane_id);
            }
        }
        if let Some(b) = bridge {
            self.pane_bridges.lock().await.insert(pane_id, b);
        }
        self.save_state().await?;
        Ok(pane_id)
    }

    pub async fn del_pane(&self, id: PaneId) -> Result<()> {
        let info = self.panes.get(id).await.ok_or(Error::PaneNotFound(id))?;
        // Drop the bridge if attached. For tmux panes this kills the real
        // tmux pane — which is exactly what `pane del` should do.
        let bridge = self.pane_bridges.lock().await.remove(&id);
        if let Some(b) = bridge {
            let _ = b.close().await;
        }
        self.pane_buffers.lock().await.remove(&id);
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
        let info = self.panes.get(pid).await.ok_or(Error::PaneNotFound(pid))?;

        if info.link_id == Some(lid) {
            return Ok(());
        }
        if let Some(existing) = info.link_id {
            return Err(Error::PaneAlreadyAttached(pid, existing));
        }
        if info.sink_handle.is_some() {
            // A tmux-owned pane reads its structure from tmux; re-binding
            // it would fork its identity. Refuse rather than corrupt.
            return Err(Error::TmuxLinkCannotBindPane);
        }

        // Q1: for direct (raw) links, auto-detach any OTHER pane already
        // attached to this link (only one may be active at a time).
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
            *arc_link.active_pane.lock().await = Some(pid);
        }

        // Build the bridge.
        let bridge: Arc<dyn PaneBridge> = match &arc_link.info.sink {
            SinkConfig::Raw => {
                let buf = self.buffer_for(pid).await;
                Arc::new(DirectBridge::new_with_buffer(Arc::clone(&arc_link.transport), buf))
            }
            SinkConfig::Tmux { .. } => {
                let tmux_arc = arc_link.tmux_sink.as_ref()
                    .ok_or_else(|| Error::Sink("tmux-backed link has no TmuxSink".into()))?
                    .clone();
                let win = first_window_with_retry(&tmux_arc).await?;
                let snap = tmux_arc.split_pane(&win.handle).await?;
                if let Some(mut i) = self.panes.get(pid).await {
                    i.sink_handle = Some(snap.handle.clone());
                    i.cols = snap.cols;
                    i.rows = snap.rows;
                    i.x = snap.x;
                    i.y = snap.y;
                    if i.window_id.is_none() {
                        i.window_id = self
                            .window_for_handle(&win.handle)
                            .await;
                    }
                    self.panes.insert(i).await;
                }
                Arc::new(TmuxBridge {
                    sink: Arc::downgrade(&tmux_arc),
                    handle: snap.handle,
                })
            }
        };

        self.pane_bridges.lock().await.insert(pid, bridge);

        if let Some(mut i) = self.panes.get(pid).await {
            i.link_id = Some(lid);
            self.panes.insert(i).await;
        }
        *arc_link.active_pane.lock().await = Some(pid);

        self.save_state().await?;
        Ok(())
    }

    /// Detach a pane from its link. Direct panes keep their buffer;
    /// tmux-owned panes cannot be detached at all.
    pub async fn detach_pane(&self, pid: PaneId) -> Result<()> {
        let info = self.panes.get(pid).await.ok_or(Error::PaneNotFound(pid))?;
        if info.sink_handle.is_some() {
            return Err(Error::TmuxPaneNotDetachable(pid));
        }
        let lid = match info.link_id {
            Some(l) => l,
            None => return Err(Error::NotAttached(pid)),
        };
        let bridge = self.pane_bridges.lock().await.remove(&pid);
        if let Some(b) = bridge {
            // P0-a: releases this pane's pump; the link's transport lives.
            let _ = b.close().await;
        }
        if let Some(arc_link) = self.links.get(lid).await {
            let mut active = arc_link.active_pane.lock().await;
            if *active == Some(pid) {
                *active = None;
            }
        }
        if let Some(mut i) = self.panes.get(pid).await {
            i.link_id = None;
            self.panes.insert(i).await;
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

    // ===== sync (v0.4.0, ADR 0008) =====

    /// Compare xscore's registry against tmux's actual structure for a
    /// tmux link. Read-only unless `heal` is set; then registry rows are
    /// corrected to match tmux. Panes tmux has but xscore never adopted
    /// are REPORTED and never adopted.
    pub async fn sync_link(&self, id: LinkId, heal: bool) -> Result<SyncReport> {
        let arc_link = self.links.get(id).await.ok_or(Error::LinkNotFound(id))?;
        let ts = match arc_link.tmux_sink.clone() {
            Some(ts) => ts,
            None => return Err(Error::NotTmuxLink(id)),
        };

        let mut report = SyncReport::default();
        let tmux_windows = ts.list_windows().await?;
        let tmux_handles: HashSet<&str> =
            tmux_windows.iter().map(|w| w.handle.as_str()).collect();

        // Scope the comparison to THIS link: the windows that hold panes
        // attached to it. Without this, `sync --heal` on link A would
        // treat link B's windows as stale and delete them.
        let link_window_ids: HashSet<WindowId> = self
            .panes
            .list_all()
            .await
            .iter()
            .filter(|p| p.link_id == Some(id))
            .filter_map(|p| p.window_id)
            .collect();

        let registry_windows: Vec<_> = self
            .windows
            .list_all()
            .await
            .into_iter()
            .filter(|w| match w.info.tmux_window_handle.as_deref() {
                // Owned by this link (holds one of its panes), or
                // currently part of this link's session. Anything else is
                // another link's business — never touch it.
                Some(h) => tmux_handles.contains(h) || link_window_ids.contains(&w.info.id),
                None => false,
            })
            .collect();

        for tw in &tmux_windows {
            if !registry_windows.iter().any(|w| {
                w.info.tmux_window_handle.as_deref() == Some(tw.handle.as_str())
            }) {
                report.missing_windows.push(tw.handle.clone());
            }
        }

        for rw in &registry_windows {
            let handle = rw.info.tmux_window_handle.clone().unwrap_or_default();
            if !tmux_handles.contains(handle.as_str()) {
                report.stale_windows.push(handle.clone());
                if heal {
                    // The tmux window is gone. Its tmux panes are gone too.
                    let pids = rw.panes.lock().await.clone();
                    for pid in pids {
                        let info = match self.panes.get(pid).await { Some(i) => i, None => continue };
                        if info.sink_handle.is_some() {
                            self.pane_bridges.lock().await.remove(&pid);
                            self.pane_buffers.lock().await.remove(&pid);
                            self.panes.remove(pid).await;
                            report.removed += 1;
                        } else if let Some(mut i) = self.panes.get(pid).await {
                            i.window_id = None;
                            self.panes.insert(i).await;
                        }
                    }
                    self.windows.remove(rw.info.id).await;
                    report.removed += 1;
                }
                continue;
            }

            // Window exists on both sides: compare/refresh.
            if heal {
                if let Some(tw) = tmux_windows.iter().find(|t| t.handle == handle) {
                    let mut updated = rw.info.clone();
                    let mut changed = false;
                    if updated.cols != tw.cols { updated.cols = tw.cols; changed = true; }
                    if updated.rows != tw.rows { updated.rows = tw.rows; changed = true; }
                    let layout = layout_from_tmux(&tw.layout);
                    if updated.layout != layout { updated.layout = layout; changed = true; }
                    if updated.name != tw.name { updated.name = tw.name.clone(); changed = true; }
                    if changed {
                        let w = Window::new(updated);
                        *w.panes.lock().await = rw.panes.lock().await.clone();
                        self.windows.insert(w).await;
                        report.healed += 1;
                    }
                }
            }

            let tmux_panes = ts.list_panes(&handle).await?;
            let tmux_pane_handles: HashSet<&str> =
                tmux_panes.iter().map(|p| p.handle.as_str()).collect();
            let registry_panes: Vec<_> = self
                .panes
                .list_all()
                .await
                .into_iter()
                .filter(|p| p.window_id == Some(rw.info.id))
                .collect();

            for tp in &tmux_panes {
                if !registry_panes.iter().any(|p| {
                    p.sink_handle.as_deref() == Some(tp.handle.as_str())
                }) {
                    report.missing_panes.push(tp.handle.clone());
                }
            }

            for rp in &registry_panes {
                let rp_handle = rp.sink_handle.clone().unwrap_or_default();
                if !tmux_pane_handles.contains(rp_handle.as_str()) {
                    report.stale_panes.push(rp_handle.clone());
                    if heal {
                        self.pane_bridges.lock().await.remove(&rp.id);
                        self.pane_buffers.lock().await.remove(&rp.id);
                        self.panes.remove(rp.id).await;
                        report.removed += 1;
                    }
                } else if heal {
                    if let Some(tp) = tmux_panes.iter().find(|t| t.handle == rp_handle) {
                        if rp.cols != tp.cols || rp.rows != tp.rows || rp.x != tp.x || rp.y != tp.y {
                            let mut i = rp.clone();
                            i.cols = tp.cols;
                            i.rows = tp.rows;
                            i.x = tp.x;
                            i.y = tp.y;
                            self.panes.insert(i).await;
                            report.healed += 1;
                        }
                    }
                }
            }
        }

        if heal {
            self.save_state().await?;
        }
        Ok(report)
    }

    // ===== internal helpers =====

    /// The per-pane buffer for a direct pane, created on first use and
    /// kept across detach/re-attach.
    async fn buffer_for(&self, pid: PaneId) -> Arc<Mutex<RingBuffer>> {
        let mut map = self.pane_buffers.lock().await;
        map.entry(pid)
            .or_insert_with(|| Arc::new(Mutex::new(RingBuffer::new(crate::backends::bridge::DIRECT_BUFFER_CAPACITY))))
            .clone()
    }

    async fn tmux_sink(&self, lid: LinkId) -> Result<Arc<TmuxSink>> {
        let arc_link = self.links.get(lid).await.ok_or(Error::LinkNotFound(lid))?;
        arc_link.tmux_sink.clone()
            .ok_or_else(|| Error::Sink("tmux-backed link has no TmuxSink".into()))
    }

    /// Which link owns a tmux window handle? Checked against tmux itself
    /// (not against session naming), so custom `?name=` sessions work.
    async fn link_for_window_handle(&self, handle: &str) -> Result<LinkId> {
        for arc_link in self.links.list().await {
            if let Some(ts) = &arc_link.tmux_sink {
                if let Ok(wins) = ts.list_windows().await {
                    if wins.iter().any(|w| w.handle == handle) {
                        return Ok(arc_link.id);
                    }
                }
            }
        }
        Err(Error::Sink(format!(
            "no tmux link owns window handle {handle}"
        )))
    }

    /// The xscore window that mirrors a tmux window handle, if registered.
    async fn window_for_handle(&self, handle: &str) -> Option<WindowId> {
        self.windows
            .list_all()
            .await
            .iter()
            .find(|w| w.info.tmux_window_handle.as_deref() == Some(handle))
            .map(|w| w.info.id)
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
            schema_version: super::state::SCHEMA_VERSION,
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
        // P0-c: with stable `%N` handles we can rebuild tmux bridges on
        // hydrate without touching tmux's structure. Direct panes cannot
        // be rebuilt (their transport died with the process) — they stay
        // orphan-ish until re-attached.
        for pinfo in mgr.panes.list_all().await {
            let (Some(lid), Some(handle)) = (pinfo.link_id, pinfo.sink_handle.clone()) else {
                continue;
            };
            let Some(arc_link) = mgr.links.get(lid).await else { continue };
            let Some(ts) = arc_link.tmux_sink.clone() else { continue };
            mgr.pane_bridges.lock().await.insert(
                pinfo.id,
                Arc::new(TmuxBridge { sink: Arc::downgrade(&ts), handle }),
            );
        }
        Ok(mgr)
    }

    async fn hydrate_link(&self, info: crate::model::LinkInfo) -> Result<()> {
        let link_id = info.id;
        let session_name = format!("xscore-{link_id}");
        let transport: Arc<dyn crate::transport::Transport> = Arc::new(NoopTransport);
        let sink: Box<dyn crate::sink::Sink> = match &info.sink {
            SinkConfig::Raw => Box::new(crate::sink::raw::RawSink),
            SinkConfig::Tmux { mode, session_name: custom } => {
                let name = custom.clone().unwrap_or_else(|| session_name.clone());
                let has = tokio::process::Command::new("tmux")
                    .args(tmux_arg_socket())
                    .args(["has-session", "-t", &name])
                    .status().await?;
                if !has.success() {
                    return Err(Error::Sink(format!(
                        "tmux session {name} gone, skipping link {link_id}"
                    )));
                }
                Box::new(TmuxSink::adopt(link_id, mode, &name))
            }
        };
        let tmux_sink = match &info.sink {
            SinkConfig::Raw => None,
            SinkConfig::Tmux { mode, session_name: custom } => {
                let name = custom.clone().unwrap_or_else(|| session_name.clone());
                Some(Arc::new(TmuxSink::adopt(link_id, mode, &name)))
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

/// tmux creates a session with one window; ask for it, tolerating the
/// brief moment where the server hasn't populated the list yet.
async fn first_window_with_retry(ts: &TmuxSink) -> Result<crate::sink::TmuxWindowSnapshot> {
    for attempt in 0..5 {
        if let Ok(windows) = ts.list_windows().await {
            if let Some(w) = windows.into_iter().next() {
                return Ok(w);
            }
        }
        if attempt < 4 {
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    }
    Err(Error::Sink(format!(
        "tmux session {} has no windows",
        ts.session_name
    )))
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
