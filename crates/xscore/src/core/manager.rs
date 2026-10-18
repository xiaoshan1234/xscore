// core/manager.rs
//
// SessionManager — the single entry point that orchestrates Link, Window, Pane,
// Backend, and the in-memory registries that route them. Maps 1:1 onto
// `xscore link {add, open, close, del}` / `xscore window {add, del, show, list}` /
// `xscore pane {add, del, show, list}` (ADR 0005 + ADR 0006).
//
// All methods are async; all errors flow through `core::Error`. The manager
// holds `Arc<...>` registries so cloning the manager is cheap and registry
// references can be shared into background tasks (snapshot, resize, etc.).

use std::sync::Arc;
use tokio::sync::Mutex;

use super::{Error, LinkAddress, Result};
use super::id::{LinkId, PaneId, WindowId};
use super::link::Link;
use super::link_registry::LinkRegistry;
use super::pane_registry::PaneRegistry;
use super::window::Window;
use super::window_registry::WindowRegistry;

use crate::backends::{BackendRegistry, PaneBackend};
use crate::backends::composite::CompositeBackend;
use crate::model::{ConnectSpec, LinkInfo, PaneInfo, SinkConfig, WindowInfo};
use crate::sink::factory::sink_for;
use crate::transport::factory as transport_factory;

#[derive(Clone)]
pub struct SessionManager {
    pub links: Arc<LinkRegistry>,
    pub windows: Arc<WindowRegistry>,
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
            windows: Arc::new(WindowRegistry::new()),
            panes: Arc::new(PaneRegistry::new()),
            backends: Arc::new(BackendRegistry::new()),
        }
    }

    // -------- add_link / open_link / close_link / del_link --------

    /// Add a new link. Always creates a brand-new external resource.
    /// Returns `(link_id, default_window_id, default_pane_id)` — auto-creates
    /// one default window inside the link's session and one default pane
    /// inside that window, so the common path stays one step (ADR 0006).
    ///
    /// The address derived from the URL (or template) is used for routing
    /// inside `LinkRegistry` only — it is no longer a user-facing dedup key
    /// (ADR 0005 retired `--link=existing`).
    pub async fn add_link(
        &self,
        spec: ConnectSpec,
        address: LinkAddress,
    ) -> Result<(LinkId, WindowId, PaneId)> {
        // Create the transport. (task-07/task-08 haven't shipped real impls
        // for shell/serial/ssh yet — the factory is a `todo!()` stub for
        // those branches; local-tmux path is fully wired and used in dev.)
        let mut transport = transport_factory::transport_for(spec.transport.clone()).await?;
        let transport_kind = transport.kind();
        let link_id = LinkId::new();

        // Build the link's info (sinks need `link_id` for `WindowInfo.link_id`,
        // so we generate `link_id` first then pass it to the sink factory).
        let link_info = LinkInfo {
            id: link_id,
            name: if spec.name.is_empty() { link_id.to_string() } else { spec.name.clone() },
            created_at: chrono::Utc::now(),
            address: address.clone(),
            transport: spec.transport.clone(),
            sink: spec.sink.clone(),
        };

        // Create the sink with the link_id (used to populate WindowInfo).
        let sink = sink_for(link_id, &spec.sink, transport_kind, transport.as_mut()).await?;
        sink.attach(transport.as_mut()).await?;

        let link = Link {
            id: link_id,
            info: link_info,
            transport,
            sink,
            windows: Mutex::new(Vec::new()),
        };
        let _arc_link = self.links.insert(link).await;

        // ADR 0006: auto-create one default window + one default pane.
        let default_window_id = self.add_window(link_id, None).await?;
        let default_pane_id = self.add_pane(default_window_id, None).await?;

        Ok((link_id, default_window_id, default_pane_id))
    }

    /// Attach to an existing tmux session (ADR 0005). Non-tmux transports
    /// return `Error::OpenOnlyForTmux(<kind>)`. For tmux: lookup the session
    /// by name in the shared `-L xscore` server (ADR 0003); if found,
    /// register a `LinkInfo` for it without creating a new tmux session; if
    /// not found, return `Error::LinkNotFound`.
    ///
    /// For this slice: stub for the tmux case. Real wiring lands when the
    /// SSH-integration phase resolves the remote tmux case; the local-tmux
    /// case is the only one wired up before that. (Window tier comes back
    /// online automatically — `link open` registers the existing session's
    /// windows into `WindowRegistry`.)
    pub async fn open_link(
        &self,
        _spec: ConnectSpec,
        address: LinkAddress,
    ) -> Result<(LinkId, WindowId, PaneId)> {
        // ADR 0005 invariant: `link open` is tmux-only. Reject non-tmux
        // transports with `Error::OpenOnlyForTmux` BEFORE the tmux-attach
        // stub panics, so callers (and integration tests) get a clean
        // error message instead of a `todo!()` panic.
        let kind = address.kind();
        if kind != "tmux" {
            return Err(Error::OpenOnlyForTmux(kind));
        }
        // The full impl enumerates the existing session's windows via
        // `tmux list-windows -t <session>` and registers them in
        // `WindowRegistry`. Until the SSH-integration phase lands, the
        // local-tmux lookup is a `todo!()`.
        todo!("stub for slice; real impl lands in tmux-attach persistence PR")
    }

    /// Detach a tmux session from the in-memory registry without killing
    /// the external resource (ADR 0005: `xscore link close` is tmux-only
    /// and the tmux session keeps running). Non-tmux transports return
    /// `Error::CloseOnlyForTmux(<kind>)`. ADR 0006 also cascades window
    /// cleanup: the windows themselves stay alive in the tmux session, but
    /// the in-memory `WindowRegistry` entries are dropped.
    pub async fn close_link(&self, id: LinkId) -> Result<()> {
        let arc_link = self.links.get(id).await.ok_or(Error::LinkNotFound(id))?;
        let kind = arc_link.info.transport_str();
        // tmux-only invariant: `SinkConfig::Tmux { mode: "off", .. }` is
        // not a tmux link. Anything else (`auto`/`local`/`remote`/etc.) is.
        let is_tmux = match &arc_link.info.sink {
            SinkConfig::Tmux { mode, .. } => mode != "off",
        };
        if !is_tmux {
            return Err(Error::CloseOnlyForTmux(kind));
        }
        // Detach semantics: remove windows + panes from in-memory registries.
        // Do NOT call sink.close() / transport.close() — the tmux session
        // (and the shared `-L xscore` server) keep running.
        let windows = arc_link.windows.lock().await.clone();
        for wid in windows {
            if let Some(arc_window) = self.windows.get(wid).await {
                let pids: Vec<PaneId> = arc_window.panes.lock().await.clone();
                for pid in pids {
                    self.backends.remove(pid).await;
                    self.panes.remove(pid).await;
                }
                self.windows.remove(wid).await;
            }
        }
        self.links.remove(id).await;
        Ok(())
    }

    /// Full teardown of a link: kill every window (cascades panes), then
    /// kill the transport + sink. For tmux, this calls
    /// `tmux -L xscore kill-session -t xscore-link-<link_uuid>` (the
    /// shared server stays up).
    pub async fn del_link(&self, id: LinkId) -> Result<()> {
        let arc_link = self.links.get(id).await.ok_or(Error::LinkNotFound(id))?;
        // Remove every window in the link. del_window does its own pane cleanup.
        let windows = arc_link.windows.lock().await.clone();
        for wid in windows {
            self.del_window(wid).await?;
        }
        // Close sink + transport. (For tmux: kill-session. For non-tmux:
        // transport-level close.)
        arc_link.sink.close().await?;
        arc_link.transport.close().await?;
        self.links.remove(id).await;
        Ok(())
    }

    // -------- add_window / del_window --------

    /// Add a new window to an existing link. Returns the new WindowId.
    /// (Was implicit in v1; promoted to a real method by ADR 0006.)
    ///
    /// Default name: `w-<first 8 hex chars of link_id>` (resolved at
    /// docs-time, task-14 §Open questions 2).
    pub async fn add_window(&self, link_id: LinkId, name: Option<String>) -> Result<WindowId> {
        let arc_link = self.links.get(link_id).await.ok_or(Error::LinkNotFound(link_id))?;
        let resolved_name = name.unwrap_or_else(|| {
            let s = arc_link.id.to_string();
            // First 8 hex chars of the link_id (UUID v7 = 32 bits of entropy
            // per first 8 hex). tmux-safe (only `[a-zA-Z0-9_-]`).
            format!("w-{}", &s[..8])
        });

        // Ask the sink to allocate the tmux window. Sink returns a
        // WindowInfo with the tmux-assigned `tmux_window_index` (and
        // placeholder `id` / `created_at`). We use the sink's index and
        // overwrite the rest with our canonical values.
        let sink_window_info = arc_link.sink.new_window(&resolved_name).await?;
        let window_id = WindowId::new();
        let canonical = WindowInfo {
            id: window_id,
            link_id,
            name: resolved_name,
            created_at: chrono::Utc::now(),
            tmux_window_index: sink_window_info.tmux_window_index,
        };

        let window = Window::new(canonical);
        self.windows.insert(window).await;
        arc_link.windows.lock().await.push(window_id);
        Ok(window_id)
    }

    /// Tear down a window. tmux kill-window kills the window; panes
    /// inside die with it. The manager unregisters the window and any
    /// panes that pointed at it.
    pub async fn del_window(&self, id: WindowId) -> Result<()> {
        let arc_window = self.windows.get(id).await.ok_or(Error::WindowNotFound(id))?;
        let link_id = arc_window.info.link_id;
        let tmux_window_index = arc_window.info.tmux_window_index;
        // Drop panes inside the window first. del_pane closes the backend
        // and removes from the PaneRegistry.
        let pids: Vec<PaneId> = arc_window.panes.lock().await.clone();
        for pid in pids {
            self.del_pane(pid).await?;
        }
        // Ask sink to kill the tmux window. (Session-mode tmux: this is
        // `tmux -L xscore kill-window -t <session>:<tmux_window_index>`.)
        if let Some(arc_link) = self.links.get(link_id).await {
            arc_link.sink.kill_window(tmux_window_index).await?;
            arc_link.windows.lock().await.retain(|w| *w != id);
        }
        self.windows.remove(id).await;
        Ok(())
    }

    // -------- add_pane / del_pane --------

    /// Add a new pane to an existing window. Returns the new PaneId.
    /// (Was: `add_pane(link_id, name)` — signature change per ADR 0006.
    /// Pane's parent is now Window, not Link.)
    pub async fn add_pane(
        &self,
        window_id: WindowId,
        name: Option<String>,
    ) -> Result<PaneId> {
        let arc_window = self.windows.get(window_id).await
            .ok_or(Error::WindowNotFound(window_id))?;
        let arc_link = self.links.get(arc_window.info.link_id).await
            .ok_or(Error::LinkNotFound(arc_window.info.link_id))?;
        // tmux split-window inside the parent window (ADR 0006).
        let sink_handle = arc_link.sink.new_pane(arc_window.info.tmux_window_index).await?;
        let pane_index = arc_window.panes.lock().await.len() as u32;
        let pane_id = PaneId::new();
        let pane_info = PaneInfo {
            id: pane_id,
            window_id,
            name: name.unwrap_or_else(|| format!("p-{pane_index}")),
            created_at: chrono::Utc::now(),
            sink_handle: sink_handle.clone(),
            pane_index,
        };
        arc_window.panes.lock().await.push(pane_id);
        self.panes.insert(pane_info).await;
        let backend: Arc<dyn PaneBackend> = Arc::new(CompositeBackend::new(
            pane_id,
            arc_link.clone(),
            sink_handle,
        ));
        self.backends.insert(pane_id, backend).await;
        Ok(pane_id)
    }

    /// Full teardown of a pane: kill the tmux split-window + remove the
    /// backend + unregister from in-memory. Was previously named
    /// `close_pane`; renamed to `del_pane` to match the CLI verb.
    /// Cross-invocation re-attach goes through `xscore link open`, not panes.
    pub async fn del_pane(&self, id: PaneId) -> Result<()> {
        let backend = self.backends.remove(id).await.ok_or(Error::PaneNotFound(id))?;
        let info = self.panes.remove(id).await.ok_or(Error::PaneNotFound(id))?;
        let window_id = info.window_id;
        // backend.close() = sink.close_pane(sink_handle). For tmux, this is
        // `tmux kill-pane -t %<pane_id>`.
        backend.close().await?;
        if let Some(arc_window) = self.windows.get(window_id).await {
            arc_window.panes.lock().await.retain(|p| *p != id);
        }
        Ok(())
    }

    // -------- backend handle --------

    /// Look up the `PaneBackend` for a given pane. Used by the `data`
    /// subcommand to do `read`/`write`/`resize` without going through the
    /// registries directly.
    pub async fn backend(&self, id: PaneId) -> Result<Arc<dyn PaneBackend>> {
        self.backends.get(id).await.ok_or(Error::PaneNotFound(id))
    }
}
