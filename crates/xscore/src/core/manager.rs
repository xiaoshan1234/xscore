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
//
// State persistence (task-15):
// - `save_state` serializes all LinkInfo to `~/.config/xscore/state/state.json`
//   after every mutating call (add_link, del_link, add_window, del_window,
//   add_pane, del_pane, close_link). Crash-safe via atomic rename.
// - `load_from_state` rebuilds the manager from a persisted snapshot by
//   re-querying the tmux server (which is the source of truth for windows
//   and panes) and adopting the existing tmux sessions for each persisted
//   link. Re-loaded links get a `NoopTransport` for I/O — the user can
//   re-run `xscore link add <url>` to get a fresh transport.

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
use crate::sink::raw::RawSink;
use crate::sink::tmux::TmuxSink;
use crate::transport::factory as transport_factory;
use crate::transport::NoopTransport;
use crate::util::tmux_arg_socket;

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

    /// Add a new link. Creates a brand-new external resource.
    /// Returns `(link_id, default_window_id, default_pane_id)` for
    /// tmux-backed links (auto-creates one default window + one
    /// default pane — ADR 0006); for raw links (direct transport
    /// I/O, no tmux server), returns `(link_id, None, None)` since
    /// there are no windows or panes to track.
    ///
    /// The address derived from the URL (or template) is used for
    /// routing inside `LinkRegistry` only — it is no longer a
    /// user-facing dedup key (ADR 0005 retired `--link=existing`).
    ///
    /// The optional `id` parameter lets the caller supply a specific
    /// `LinkId` (parsed from a uuid v7 string). If `Some(id)` and the
    /// id is already in use, returns `Error::LinkIdExists(id)`. If
    /// `Some(id)` and the id is free, uses that id verbatim. If
    /// `None`, generates a fresh uuid v7 (the default behavior).
    /// Useful for scripted workflows and for reconnect-style flows
    /// that want a known id.
    pub async fn add_link(
        &self,
        spec: ConnectSpec,
        address: LinkAddress,
        id: Option<LinkId>,
    ) -> Result<(LinkId, Option<WindowId>, Option<PaneId>)> {
        // Create the transport.
        let mut transport = transport_factory::transport_for(spec.transport.clone()).await?;
        let transport_kind = transport.kind();

        // Resolve the link id: user-supplied (if free) or auto-generated.
        let link_id = match id {
            Some(supplied) if self.links.get(supplied).await.is_some() => {
                return Err(Error::LinkIdExists(supplied));
            }
            Some(supplied) => supplied,   // free → use as-is
            None => LinkId::new(),        // default → auto-gen
        };

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
        // The session name is derived from `link_id` (`xscore-<link_id>`)
        // so hydration can re-discover the tmux session on restart.
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

        // For tmux-backed links: ADR 0006 auto-creates one default
        // window + one default pane. `TmuxSink::create` already ran
        // `tmux new-session -d -s <name>`, which always creates
        // exactly ONE window at index 0; we register THAT window in
        // the manager directly (skipping the previous double-window
        // bug where we ALSO called `add_window`).
        //
        // For raw links (default): no tmux session, no windows,
        // no panes — return (link_id, None, None).
        let (default_window_id, default_pane_id) = match &spec.sink {
            SinkConfig::Raw => (None, None),
            SinkConfig::Tmux { .. } => {
                let wid = self.register_window_at(link_id, 0).await?;
                let pid = self.add_pane(wid, None).await?;
                (Some(wid), Some(pid))
            }
        };

        self.save_state().await?;
        Ok((link_id, default_window_id, default_pane_id))
    }

    /// Register a window in the manager at a given tmux-assigned index
    /// WITHOUT making a new-window tmux call. Used by `add_link` to
    /// adopt the auto-created default window from `TmuxSink::create`'s
    /// `tmux new-session`. Also used by `hydrate` to re-register
    /// persisted windows.
    async fn register_window_at(&self, link_id: LinkId, tmux_window_index: u32) -> Result<WindowId> {
        let arc_link = self.links.get(link_id).await.ok_or(Error::LinkNotFound(link_id))?;
        let resolved_name = format!("w-{}", &link_id.to_string()[..8]);
        let window_id = WindowId::new();
        let canonical = WindowInfo {
            id: window_id,
            link_id,
            name: resolved_name,
            created_at: chrono::Utc::now(),
            tmux_window_index,
        };
        let window = Window::new(canonical);
        self.windows.insert(window).await;
        arc_link.windows.lock().await.push(window_id);
        Ok(window_id)
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
    ) -> Result<(LinkId, Option<WindowId>, Option<PaneId>)> {
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
        // ADR 0005: `link close` is for tmux sessions only. The
        // "tmux-ness" of a link is determined by its transport/address
        // (the kind of resource the link wraps), NOT by its sink mode
        // (which is tmux-by-default for shell/serial/ssh too — but
        // the link isn't a "tmux session" unless the URL was tmux://).
        if kind != "tmux" {
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
        self.save_state().await?;
        Ok(())
    }

    /// Full teardown of a link: kill every window (cascades panes), then
    /// kill the transport + sink. For tmux, this calls
    /// `tmux -L xscore kill-session -t xscore-<link_uuid>` (the
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
        self.save_state().await?;
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
        // Raw links (no tmux server) have no concept of windows.
        // The user must opt in to tmux with `?tmux=on` in the URL
        // to use the window/pane subcommands.
        if matches!(arc_link.info.sink, SinkConfig::Raw) {
            return Err(Error::NotTmuxLink(link_id));
        }
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
        self.save_state().await?;
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
        // RawSink's `kill_window` is `unreachable!()` — guard above
        // prevents reaching this for raw links.
        if let Some(arc_link) = self.links.get(link_id).await {
            arc_link.sink.kill_window(tmux_window_index).await?;
            arc_link.windows.lock().await.retain(|w| *w != id);
        }
        self.windows.remove(id).await;
        self.save_state().await?;
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
        // Raw links have no tmux session, so no panes either.
        if matches!(arc_link.info.sink, SinkConfig::Raw) {
            return Err(Error::NotTmuxLink(arc_window.info.link_id));
        }
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
        self.save_state().await?;
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
        self.save_state().await?;
        Ok(())
    }

    // -------- backend handle --------

    /// Look up the `PaneBackend` for a given pane. Used by the `data`
    /// subcommand to do `read`/`write`/`resize` without going through the
    /// registries directly.
    pub async fn backend(&self, id: PaneId) -> Result<Arc<dyn PaneBackend>> {
        self.backends.get(id).await.ok_or(Error::PaneNotFound(id))
    }

    // -------- state persistence (task-15) --------

    /// Serialize all current `LinkInfo`s + their windows + panes to the
    /// state snapshot file. Called automatically after every mutating
    /// method; can also be called explicitly from `main` before exit for
    /// paranoia. Crash-safe: writes to `state.json.tmp` then renames
    /// atomically.
    pub async fn save_state(&self) -> Result<()> {
        let arc_links = self.links.list().await;
        let mut snapshots = Vec::new();
        for arc_link in &arc_links {
            let link_id = arc_link.id;
            // Collect this link's windows.
            let arc_windows = self.windows.list_for_link(link_id).await;
            let windows: Vec<WindowInfo> = arc_windows
                .iter()
                .map(|w| w.info.clone())
                .collect();
            // Collect this link's panes (by walking each window's pane list
            // — PaneRegistry doesn't have a list_for_link helper).
            let mut panes: Vec<PaneInfo> = Vec::new();
            for arc_window in &arc_windows {
                for pane_id in arc_window.panes.lock().await.iter() {
                    if let Some(p) = self.panes.get(*pane_id).await {
                        panes.push(p);
                    }
                }
            }
            snapshots.push(super::state::LinkSnapshot {
                link: arc_link.info.clone(),
                windows,
                panes,
            });
        }
        tokio::task::spawn_blocking(move || super::state::save(&snapshots))
            .await
            .map_err(|e| Error::Config(format!("join: {e}")))?
    }

    /// Load the state snapshot from disk (if any) and hydrate a new
    /// `SessionManager` by adopting the existing tmux sessions for each
    /// persisted link. Returns an empty manager if no snapshot exists.
    ///
    /// Hydration algorithm per `LinkSnapshot`:
    /// 1. Construct the tmux session name: `xscore-<link_id>`.
    /// 2. Verify the session exists via `tmux has-session -t <session>`.
    ///    If it doesn't (e.g. user killed it manually), skip the link.
    /// 3. Adopt the tmux session via `TmuxSink::adopt` (no new-session
    ///    call — the existing session is reused).
    /// 4. Use `NoopTransport` for I/O (the user can re-add the link to
    ///    get a fresh transport for data ops).
    /// 5. Register each persisted window in `WindowRegistry` with its
    ///    ORIGINAL xscore WindowId (so cross-invocation `window list`
    ///    shows the same ids the user saved).
    /// 6. Register each persisted pane in `PaneRegistry` + `BackendRegistry`,
    ///    and add to the parent window's pane list.
    pub async fn load_from_state() -> Result<Self> {
        match super::state::load()? {
            Some(s) => Self::hydrate(s).await,
            None => Ok(Self::new()),
        }
    }

    async fn hydrate(snapshots: Vec<super::state::LinkSnapshot>) -> Result<Self> {
        let mgr = Self::new();
        for snap in snapshots {
            // Best-effort: skip links that fail hydration (e.g. tmux
            // session killed externally).
            if let Err(e) = mgr.hydrate_one(snap).await {
                eprintln!("hydration: skipping link: {e}");
            }
        }
        Ok(mgr)
    }

    async fn hydrate_one(&self, snap: super::state::LinkSnapshot) -> Result<()> {
        let info = snap.link.clone();
        let link_id = info.id;
        let session_name = format!("xscore-{link_id}");

        // For raw (non-tmux) links, skip the tmux session check and
        // adopt the link with a RawSink. The original transport
        // process is gone (re-add the link to get a fresh transport
        // for data ops); the sink is just a marker.
        //
        // For tmux links, verify the tmux session still exists. If
        // not, skip the link (it was killed externally).
        let sink: Box<dyn crate::sink::Sink> = match &info.sink {
            SinkConfig::Raw => Box::new(RawSink),
            SinkConfig::Tmux { .. } => {
                let has = tokio::process::Command::new("tmux")
                    .args(tmux_arg_socket())
                    .args(["has-session", "-t", &session_name])
                    .status().await?;
                if !has.success() {
                    eprintln!("hydration: tmux session {session_name} gone, skipping");
                    return Ok(());
                }
                let sink_mode = match &info.sink {
                    SinkConfig::Tmux { mode, .. } => mode.as_str(),
                    SinkConfig::Raw => unreachable!(),
                };
                Box::new(TmuxSink::adopt(link_id, sink_mode, &session_name))
            }
        };

        // Use a no-op transport — the user can re-add the link to get
        // a real transport. The tmux sink is the source of truth for
        // the sink side; data ops won't work until the link is re-added.
        let transport: Box<dyn crate::transport::Transport> = Box::new(NoopTransport);

        // Build a Link and insert it.
        let link = Link {
            id: link_id,
            info,
            transport,
            sink,
            windows: Mutex::new(Vec::new()),
        };
        let arc_link = self.links.insert(link).await;

        // Re-register windows + panes from the snapshot, preserving
        // their ORIGINAL xscore ids. (Raw links have no windows or
        // panes — both lists are empty.)
        let mut window_ids: Vec<WindowId> = Vec::new();
        for winfo in snap.windows {
            let wid = winfo.id;
            let window = Window::new(winfo);
            self.windows.insert(window).await;
            arc_link.windows.lock().await.push(wid);
            window_ids.push(wid);
        }
        for pinfo in snap.panes {
            let pid = pinfo.id;
            // Find the parent window.
            if let Some(arc_window) = self.windows.get(pinfo.window_id).await {
                arc_window.panes.lock().await.push(pid);
            }
            self.panes.insert(pinfo.clone()).await;
            let backend: Arc<dyn PaneBackend> = Arc::new(CompositeBackend::new(
                pid,
                arc_link.clone(),
                pinfo.sink_handle,
            ));
            self.backends.insert(pid, backend).await;
        }
        // silence unused var warning
        let _ = window_ids;
        Ok(())
    }
}
