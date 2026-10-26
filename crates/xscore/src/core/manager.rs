// core/manager.rs (v0.6.0 — Session owns the backend, ADR 0010)
//
// All methods are async; all errors flow through `core::Error`. The
// manager holds `Arc<...>` registries so cloning the manager is cheap.
//
// Creation semantics:
//   - `add_link(spec, addr, id, pane)` yields a `Pipe` Link, an independent
//     Buffer, and a usable set of UI rows:
//       * `pane: Some(pid)` → binds that existing (hollow) pane;
//       * `pane: None`      → creates a native window + a pane and its
//                             and its direct runtime bridge.
//     There is no tmux branch: a tmux session is created/adopted at the
//     session level (`create_tmux_session` / `adopt_tmux_session`).
//   - `add_window_in(session, ..)` creates a real tmux window through the
//     session's control client when the session is tmux-backed, else a native
//     window.
//   - `add_pane_in(.., window)` splits inside a real tmux window (through the
//     control client) when the window has a `WindowBackend::Tmux`, else it
//     inserts a hollow pane.
//
// `detach_pane` clears only the UI's Buffer reference and never touches the
// transport. `del_link` on a `Pipe` Link stops its SessionManager route and
// keeps Buffers and Pane rows; a `Control` link cannot be
// deleted at all — `session kill` / `session detach` are the verbs.
//
// v0.6.0 PR3b (ADR 0010 §D2) added tmux-backed sessions: `create_tmux_session`
// / `adopt_tmux_session` give a session ONE `tmux -CC attach` client, which is
// the session's single `Control` link. The Tmux SessionManager maps `%output
// %N` to a Buffer and translates structure events into registry actions.
// PR4 deleted the old per-command tmux path, so the control client is the only
// way xscore touches tmux data.
//
// v0.6.0 PR5 (ADR 0009 §1): the client — and every pane buffer it fills —
// lives in the **daemon** when one is configured, so `data read` is a stream
// across CLI invocations rather than a `capture-pane` snapshot. This module
// therefore has two modes, and says so at each decision point:
//
//   * daemon mode (`main.rs` always enables it): `OpenTmux`/`Structure`/
//     `TmuxCommand`/`Attach {key}`/`CloseTmux` on the daemon; no client, no
//     pump and no buffer of ours. Structure is *enumerated on touch*.
//   * in-process mode (`data_smoke`, an embedding caller): exactly what PR3b
//     built — spawn the client here, pump `%output` into `buffer_memory`, react
//     to events. Kept behind `daemon.is_none()` so the tests keep covering the
//     tmux protocol itself, without a socket in the way.
//
// State persistence: `save_state` writes `StateSnapshot` (schema v3);
// see `core::state` for the version gate and the migrations.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use tokio::sync::Mutex;

use super::buffer::{RingBuffer, DIRECT_BUFFER_CAPACITY};
use super::buffer_registry::BufferRegistry;
use super::id::{BufferId, LinkId, PaneId, SessionId, WindowId};
use super::link::Link;
use super::link_registry::LinkRegistry;
use super::pane_registry::PaneRegistry;
use super::session::Session;
use super::session_managers::{SessionManager as SessionManagerRuntime, TmuxEventAction};
use super::session_registry::SessionRegistry;
use super::window::Window;
use super::window_registry::WindowRegistry;
use super::{Error, LinkAddress, Result};

use crate::backends::{ControlBridge, DaemonBridge, DirectBridge, PaneBridge};
use crate::daemon::client::DaemonClient;
use crate::model::{
    BufferInfo, BufferOrigin, ConnectSpec, EndpointRef, PaneInfo, SessionBackend, SessionInfo,
    TmuxHostConfig, TmuxSocket, TransportConfig, WindowBackend, WindowInfo,
};
use crate::tmux::{PaneLine, TmuxControl, TmuxEvent, WindowLine};
use crate::transport::factory as transport_factory;
use crate::transport::NoopTransport;

/// What `link add` produced: a Link, its independent Buffer and the initial UI
/// Pane/Window that reference it.
#[derive(Debug, Clone, Copy)]
pub struct LinkAddResult {
    pub link_id: LinkId,
    pub buffer_id: BufferId,
    pub window_id: Option<WindowId>,
    pub pane_id: PaneId,
}

#[derive(Clone)]
pub struct SessionManager {
    /// v0.6.0 (ADR 0010 §1): the workspaces. Children point at a session by
    /// id; this registry is the only place the names live.
    pub sessions: Arc<SessionRegistry>,
    pub links: Arc<LinkRegistry>,
    pub windows: Arc<WindowRegistry>,
    pub panes: Arc<PaneRegistry>,
    pub buffers: Arc<BufferRegistry>,
    session_managers: Arc<Mutex<HashMap<SessionId, SessionManagerRuntime>>>,
    /// Per-Buffer bridge map. Stored separately from `PaneInfo` so mutating
    /// the info (which is JSON-serializable) doesn't destroy the bridge.
    /// Bridges are in-memory only.
    buffer_bridges: Arc<Mutex<HashMap<BufferId, Arc<dyn PaneBridge>>>>,
    /// Per-pane direct-pane buffers. They OUTLIVE the bridge: detaching a
    /// direct pane drops the bridge but keeps the history (ADR 0008).
    /// Only used in in-process mode — with a daemon, the buffer belongs
    /// to the daemon (ADR 0009).
    buffer_memory: Arc<Mutex<HashMap<BufferId, Arc<Mutex<RingBuffer>>>>>,
    /// v0.5.0 (ADR 0009): when set, direct links are daemon-backed —
    /// their transport and pane buffers live in the resident daemon, so
    /// they survive this process. `None` keeps everything in-process
    /// (that is what `data_smoke` exercises).
    daemon: Option<Arc<DaemonClient>>,
    /// v0.6.0 (ADR 0010 §D2): the live control client of every tmux-backed
    /// session this process attached, **in-process mode only**. With a daemon
    /// the client belongs to the daemon (`tmux: HashMap<SessionId,
    /// ControlSlot>` there), because a client that dies with the CLI cannot
    /// stream anything into the next one. One per session — a tmux session is
    /// driven by exactly one client, and a second subscriber for the same
    /// `%output %N` would append every byte twice.
    tmux_controls: Arc<Mutex<HashMap<SessionId, Arc<TmuxControl>>>>,
    /// Serialises structure reconciliation. The event pump and a command can
    /// both reconcile at once (a `%window-add` arriving during `window ls`),
    /// and two reconciles racing would insert two rows for one `%N`.
    tmux_reconcile: Arc<Mutex<()>>,
    /// Serialises *attaching*. Two clients on one tmux session would mean two
    /// subscribers for the same `%output %N`, i.e. every byte appended twice —
    /// the one-mechanism rule, one level down.
    tmux_attach: Arc<Mutex<()>>,
}

/// Rest of what `create_tmux_session` needs beyond the five structural
/// arguments: what the first pane runs, and where.
#[derive(Debug, Clone, Default)]
pub struct TmuxCreateOptions {
    /// A command line, run through `sh -c` (tmux execs its trailing argv
    /// directly, so `--cmd 'cat -v'` would otherwise look for a program with a
    /// space in its name).
    pub command: Option<String>,
    /// The pane's working directory (`tmux new-session -c`).
    pub cwd: Option<String>,
}

impl Default for SessionManager {
    fn default() -> Self {
        Self::new()
    }
}

impl SessionManager {
    pub fn new() -> Self {
        Self {
            sessions: Arc::new(SessionRegistry::new()),
            links: Arc::new(LinkRegistry::new()),
            windows: Arc::new(WindowRegistry::new()),
            panes: Arc::new(PaneRegistry::new()),
            buffers: Arc::new(BufferRegistry::new()),
            session_managers: Arc::new(Mutex::new(HashMap::new())),
            buffer_bridges: Arc::new(Mutex::new(HashMap::new())),
            buffer_memory: Arc::new(Mutex::new(HashMap::new())),
            daemon: None,
            tmux_controls: Arc::new(Mutex::new(HashMap::new())),
            tmux_reconcile: Arc::new(Mutex::new(())),
            tmux_attach: Arc::new(Mutex::new(())),
        }
    }

    /// v0.5.0 (ADR 0009): back direct links with the resident daemon.
    ///
    /// The CLI turns this on; the in-process tests do not. With it on,
    /// `add_link` / `attach_pane` send the transport to the daemon and
    /// install a `DaemonBridge`, so the link keeps working after this
    /// process exits — which is the whole reason v0.5.0 exists.
    pub fn enable_daemon(&mut self, client: Arc<DaemonClient>) {
        self.daemon = Some(client);
    }

    /// v0.5.0: is this link's transport alive right now?
    ///
    /// tmux (`Control`) links: the link's liveness is the session's — it is the
    /// one connection to the server, so "is the link up" and "is the session
    /// streaming" are the same question (arch.md §4).
    /// pipe links: ask the daemon. Without a daemon (in-process mode) fall back
    /// to the flag recorded at construction/hydration time.
    pub async fn link_is_connected(&self, lid: LinkId) -> bool {
        let Some(link) = self.links.get(lid).await else {
            return false;
        };
        if link.info.is_control() {
            let sid = link.info.session_id;
            // v0.6.0 PR5: the daemon owns the session's client, so its
            // liveness IS the answer (ADR 0010 §4: the link is the connection).
            // A daemon that is not running holds no client, but that says
            // nothing about the tmux session — fall through to the server.
            // (With a daemon that IS running but holds no client for this
            // session, `connected` is false: nothing is streaming, and asking
            // the server would put a `has-session` exec behind every `link
            // show`, which is what moving the client here was meant to end.)
            if self.daemon.is_some() {
                if let Ok(alive) = crate::daemon::client::probe_tmux(sid).await {
                    return alive;
                }
            }
            // In-process: the attached client, then the session-keyed map (a
            // stale handle closed by `%exit` must not count as connected).
            if let Some(control) = &link.control {
                if !control.is_closed() {
                    return true;
                }
            }
            let held = { self.tmux_controls.lock().await.get(&sid).cloned() };
            if let Some(control) = held {
                return !control.is_closed();
            }
            // No client in this process. Asking the server (one `has-session`)
            // is the honest answer; reporting `false` would call a perfectly
            // healthy session disconnected just because this process did not
            // attach. This is a `show`, and showing is live (arch.md §6).
            let Ok(info) = self.get_session(sid).await else {
                return false;
            };
            return match info.backend {
                SessionBackend::Tmux {
                    host,
                    socket,
                    session,
                } => self
                    .tmux_session_exists(&host, &socket, &session)
                    .await
                    .unwrap_or(false),
                SessionBackend::Native => false,
            };
        }
        match &self.daemon {
            Some(_) => crate::daemon::client::probe_link(lid)
                .await
                .unwrap_or(false),
            None => link.connected,
        }
    }

    // ===== sessions (v0.6.0, ADR 0010 §1) =====

    /// Which workspace does this command mean?
    ///
    /// `Some(name)` is the user being explicit (`--session work`) and must
    /// never fall back to something else — a typo would silently write into the
    /// wrong workspace. `None` picks the only existing session. With no
    /// sessions, the caller must create one; with multiple sessions, the
    /// caller must be explicit. Child commands never invent a container.
    pub async fn resolve_session(&self, name: Option<&str>) -> Result<SessionId> {
        if let Some(n) = name {
            return self
                .sessions
                .get_by_name(n)
                .await
                .map(|s| s.info.id)
                .ok_or_else(|| Error::SessionNotFound(n.to_string()));
        }
        let all = self.sessions.list().await;
        if all.len() == 1 {
            return Ok(all[0].info.id);
        }
        if all.is_empty() {
            return Err(Error::NoSession);
        }
        Err(Error::SessionAmbiguous)
    }

    pub async fn add_session_native(&self, name: &str) -> Result<SessionId> {
        self.add_session(SessionInfo::new_native(name)).await
    }

    /// Every child references a session, and hydration drops rows whose
    /// session is missing (with a warning), so writing one here would be silent
    /// data loss on the next load. A caller passing an unknown id is a bug.
    async fn require_session(&self, id: SessionId) -> Result<()> {
        if self.sessions.get(id).await.is_none() {
            return Err(Error::SessionNotFound(id.to_string()));
        }
        Ok(())
    }

    pub async fn add_session(&self, info: SessionInfo) -> Result<SessionId> {
        if self.sessions.get_by_name(&info.name).await.is_some() {
            return Err(Error::SessionExists(info.name));
        }
        let id = info.id;
        self.sessions.insert(Session::new(info)).await;
        self.save_state().await?;
        Ok(id)
    }

    /// Sessions in creation order (uuid v7 is chronological, but
    /// `created_at` is what the CLI prints, so sort by that).
    pub async fn list_sessions(&self) -> Result<Vec<SessionInfo>> {
        let mut all: Vec<SessionInfo> = self
            .sessions
            .list()
            .await
            .iter()
            .map(|s| s.info.clone())
            .collect();
        all.sort_by_key(|s| s.created_at);
        Ok(all)
    }

    pub async fn get_session(&self, id: SessionId) -> Result<SessionInfo> {
        self.sessions
            .get(id)
            .await
            .map(|s| s.info.clone())
            .ok_or_else(|| Error::SessionNotFound(id.to_string()))
    }

    pub async fn rename_session(&self, id: SessionId, new_name: &str) -> Result<()> {
        let current = self
            .sessions
            .get(id)
            .await
            .ok_or_else(|| Error::SessionNotFound(id.to_string()))?;
        if current.info.name == new_name {
            return Ok(());
        }
        if self.sessions.get_by_name(new_name).await.is_some() {
            return Err(Error::SessionExists(new_name.to_string()));
        }
        let mut info = current.info.clone();
        info.name = new_name.to_string();
        // Remove + reinsert so `by_name` follows the new name (the registry's
        // `remove` only drops the index entry still pointing at this id).
        self.sessions.remove(id).await;
        self.sessions.insert(Session::new(info)).await;
        self.save_state().await?;
        Ok(())
    }

    /// Destroy a session and everything in it — the only cascade in the model
    /// (arch.md §8).
    ///
    /// For a tmux-backed session the backend dies first: the control client is
    /// closed, then `kill-session` takes the server session down. After that
    /// the rows are swept exactly like a native session's (orphan panes and
    /// empty windows, then the session row last), so an interrupted kill
    /// leaves an empty workspace rather than children pointing at a missing
    /// session.
    ///
    /// Links go through `del_link` — except a `Control` link, which `del_link`
    /// refuses on purpose (ADR 0010 §D5) and whose row is simply dropped here.
    pub async fn del_session(&self, id: SessionId) -> Result<()> {
        let session = self
            .sessions
            .get(id)
            .await
            .ok_or_else(|| Error::SessionNotFound(id.to_string()))?;

        if let SessionBackend::Tmux {
            host,
            socket,
            session: name,
        } = &session.info.backend
        {
            // Destroy means destroy: the control client goes first (so nothing
            // is streaming into rows that are about to disappear), then
            // `kill-session` takes the server session down. Both steps are best
            // effort — a session that is already gone must still lose its rows.
            //
            // v0.6.0 PR5: in daemon mode the client (and its buffers) is the
            // daemon's, so `CloseTmux` is the whole teardown; `kill-session`
            // stays a one-shot exec (arch.md §6 allows exactly that shape).
            match &self.daemon {
                Some(client) => {
                    let _ = client.close_tmux(id).await;
                }
                None => {
                    let control = { self.tmux_controls.lock().await.remove(&id) };
                    if let Some(control) = control {
                        let _ = control.close().await;
                    }
                }
            }
            let _ = self
                .run_tmux(
                    host,
                    socket,
                    &["kill-session".to_string(), "-t".to_string(), name.clone()],
                )
                .await;
        }

        let link_ids: Vec<LinkId> = self
            .links
            .list()
            .await
            .iter()
            .filter(|l| l.info.session_id == id)
            .map(|l| l.id)
            .collect();
        for lid in link_ids {
            let is_control = match self.links.get(lid).await {
                Some(link) => link.info.is_control(),
                None => continue,
            };
            if is_control {
                // Nothing to tear down: the client is closed and its panes are
                // swept below. `del_link` would refuse it anyway.
                self.links.remove(lid).await;
                continue;
            }
            self.del_link(lid).await?;
        }

        let straggling_buffers: Vec<BufferInfo> = self
            .buffers
            .list_all()
            .await
            .into_iter()
            .filter(|buffer| buffer.session_id == id)
            .collect();
        for info in &straggling_buffers {
            let bridge = self.buffer_bridges.lock().await.remove(&info.id);
            self.buffer_memory.lock().await.remove(&info.id);
            // Releasing a bridge only stops a pump: a direct bridge's `close`
            // is "stop my reader", a control bridge's is a no-op, and the tmux
            // session is already gone either way.
            if let Some(b) = bridge {
                let _ = b.close().await;
            }
            self.buffers.remove(info.id).await;
            if let Some(client) = &self.daemon {
                let _ = client.delete_buffer(info.id).await;
            }
        }
        let pane_ids: Vec<PaneId> = self
            .panes
            .list_all()
            .await
            .into_iter()
            .filter(|pane| pane.session_id == id)
            .map(|pane| pane.id)
            .collect();
        for pid in pane_ids {
            self.panes.remove(pid).await;
        }

        let doomed_windows: Vec<WindowId> = self
            .windows
            .list_all()
            .await
            .iter()
            .filter(|w| w.info.session_id == id)
            .map(|w| w.info.id)
            .collect();
        for wid in doomed_windows {
            self.windows.remove(wid).await;
        }

        self.sessions.remove(id).await;
        self.session_managers.lock().await.remove(&id);
        self.save_state().await?;
        Ok(())
    }

    /// Stop managing a session without touching its backend (arch.md §8).
    ///
    /// Only meaningful for tmux: the tmux server keeps running and the session
    /// stays in `tmux ls`; xscore just drops its rows. A native session has no
    /// backend outside this process, so "detach" would silently mean "kill" —
    /// refuse instead.
    pub async fn detach_session(&self, id: SessionId) -> Result<()> {
        let session = self
            .sessions
            .get(id)
            .await
            .ok_or_else(|| Error::SessionNotFound(id.to_string()))?;
        if !session.info.is_tmux() {
            return Err(Error::DetachNativeSession(id));
        }

        // Detach: the control client is closed (in daemon mode: `CloseTmux`,
        // which drops the slot and therefore its buffers), and tmux keeps
        // running (arch.md §8, "session detach leaves the backend running").
        match &self.daemon {
            Some(client) => {
                let _ = client.close_tmux(id).await;
            }
            None => {
                let control = { self.tmux_controls.lock().await.remove(&id) };
                if let Some(control) = control {
                    let _ = control.close().await;
                }
            }
        }

        // No `del_link`, no `close()`: both would terminate something that
        // belongs to tmux. Dropping the rows (and the bridges) is the whole
        // operation.
        let link_ids: Vec<LinkId> = self
            .links
            .list()
            .await
            .iter()
            .filter(|l| l.info.session_id == id)
            .map(|l| l.id)
            .collect();
        for lid in link_ids {
            self.links.remove(lid).await;
        }
        let buffers: Vec<BufferId> = self
            .buffers
            .list_all()
            .await
            .into_iter()
            .filter(|buffer| buffer.session_id == id)
            .map(|buffer| buffer.id)
            .collect();
        for buffer_id in buffers {
            self.buffer_bridges.lock().await.remove(&buffer_id);
            self.buffer_memory.lock().await.remove(&buffer_id);
            self.buffers.remove(buffer_id).await;
        }
        let pane_ids: Vec<PaneId> = self
            .panes
            .list_all()
            .await
            .iter()
            .filter(|p| p.session_id == id)
            .map(|p| p.id)
            .collect();
        for pid in pane_ids {
            self.panes.remove(pid).await;
        }
        let window_ids: Vec<WindowId> = self
            .windows
            .list_all()
            .await
            .iter()
            .filter(|w| w.info.session_id == id)
            .map(|w| w.info.id)
            .collect();
        for wid in window_ids {
            self.windows.remove(wid).await;
        }
        self.sessions.remove(id).await;
        self.session_managers.lock().await.remove(&id);
        self.save_state().await?;
        Ok(())
    }

    // ===== tmux-backed sessions (v0.6.0, ADR 0010 §D2/§D7) =====
    //
    // A tmux-backed session is a real tmux session plus ONE control client,
    // which is the session's single link. Everything below exists to keep the
    // three things that can disagree in step:
    //
    //   * the server (owner of the structure and of every pane's bytes);
    //   * the registries (`WindowInfo` mirrors `@N`, `PaneInfo` mirrors `%N`);
    //   * independent Buffers, filled from `%output %N` and routed by the Tmux
    //     SessionManager's `%N -> BufferId` map.
    //
    // Where the client lives is the one thing PR5 changed: with a daemon it is
    // the daemon's (so the buffers outlive this process and `data read` is a
    // stream), without one it is this process's. Structure is therefore
    // **enumerated on touch**: a one-shot CLI has no event subscription between
    // commands, so a listing asks for the structure through whichever client
    // holds it. Events still drive the in-process pumps (and liveness, in the
    // daemon: `%exit` flips the slot's `alive` flag).

    /// Create a real tmux session and manage it.
    ///
    /// The tmux session is named after the xscore session: two names for one
    /// thing would drift, and `session attach` is the verb for a session that
    /// already exists under its own name.
    pub async fn create_tmux_session(
        &self,
        name: &str,
        host: TmuxHostConfig,
        socket: TmuxSocket,
        cols: u16,
        rows: u16,
    ) -> Result<SessionId> {
        self.create_tmux_session_with(name, host, socket, cols, rows, TmuxCreateOptions::default())
            .await
    }

    /// `create_tmux_session` plus the first pane's command and directory.
    pub async fn create_tmux_session_with(
        &self,
        name: &str,
        host: TmuxHostConfig,
        socket: TmuxSocket,
        cols: u16,
        rows: u16,
        opts: TmuxCreateOptions,
    ) -> Result<SessionId> {
        self.require_free_session_name(name).await?;
        // `has-session` exits 1 when the session is absent — that is the answer
        // we want, not an error.
        if self.tmux_session_exists(&host, &socket, name).await? {
            return Err(Error::TmuxSessionExists(name.to_string()));
        }

        let mut args: Vec<String> = vec![
            "new-session".into(),
            "-d".into(),
            "-s".into(),
            name.into(),
            "-x".into(),
            cols.to_string(),
            "-y".into(),
            rows.to_string(),
        ];
        if let Some(cwd) = opts.cwd.as_deref().filter(|c| !c.is_empty()) {
            args.push("-c".into());
            args.push(cwd.to_string());
        }
        if let Some(command) = opts.command.as_deref().filter(|c| !c.trim().is_empty()) {
            args.extend([
                "--".into(),
                "/bin/sh".into(),
                "-c".into(),
                command.to_string(),
            ]);
        }
        let out = self.run_tmux(&host, &socket, &args).await?;
        if !out.status.success() {
            return Err(self.tmux_failure(&host, name, "new-session", &out));
        }
        self.finish_tmux_session(name, host, socket, true).await
    }

    /// Take over a tmux session that already exists (ADR 0008, now through the
    /// control client instead of one exec per command).
    pub async fn adopt_tmux_session(
        &self,
        name: &str,
        host: TmuxHostConfig,
        socket: TmuxSocket,
    ) -> Result<SessionId> {
        self.require_free_session_name(name).await?;
        if !self.tmux_session_exists(&host, &socket, name).await? {
            return Err(Error::TmuxTargetGone {
                host: crate::tmux::host_label(&host),
                session: name.to_string(),
            });
        }
        // Adoption never creates and never kills: the session was the user's
        // before it was ours.
        self.finish_tmux_session(name, host, socket, false).await
    }

    /// Re-read a tmux session's structure now.
    ///
    /// Listings call this because a CLI invocation is one process per command:
    /// between commands nothing of ours is listening, so a window created
    /// outside xscore is invisible until something asks. In daemon mode that is
    /// one `Structure` request to the resident client; in-process it is an
    /// attach for the lifetime of this command (arch.md §7).
    pub async fn refresh_tmux_session(&self, id: SessionId) -> Result<()> {
        // A session whose rows came from `state migrate` has no control link
        // yet; reconciliation records every pane against it.
        self.ensure_control_link(id).await?;
        match &self.daemon {
            Some(client) => match client.structure(id).await {
                Ok(structure) => {
                    let view = TmuxView::from_structure(structure);
                    self.reconcile_all(id, &view, None).await.map(|_| ())
                }
                // The daemon holds no client yet (it restarted, or this is the
                // first command since boot). Opening one enumerates as well,
                // so both paths end in the same rows.
                Err(_) => self.attach_daemon(id).await,
            },
            None => {
                let control = self.ensure_local_control(id).await?;
                let view = self.view_with(&control, id).await?;
                self.reconcile_all(id, &view, Some(&control))
                    .await
                    .map(|_| ())
            }
        }
    }

    /// The session's control client **in this process**, attached on demand.
    ///
    /// Only used when there is no daemon (`data_smoke`, an embedding caller).
    /// With a daemon the client is the daemon's (`OpenTmux`), so this is never
    /// reached — `attach_daemon` is its counterpart.
    ///
    /// Attaching is also the reconnect path: after `%exit` (the client died,
    /// the ssh channel dropped) the handle is closed, so it is dropped and
    /// replaced. Output produced while nothing was attached is gone — tmux
    /// replays nothing (ADR 0010 §D2) — but the session itself keeps running.
    async fn ensure_local_control(&self, id: SessionId) -> Result<Arc<TmuxControl>> {
        // Held across the check-and-spawn: see `tmux_attach`.
        let _guard = self.tmux_attach.lock().await;
        let existing = { self.tmux_controls.lock().await.get(&id).cloned() };
        if let Some(control) = existing {
            if !control.is_closed() {
                return Ok(control);
            }
            self.tmux_controls.lock().await.remove(&id);
        }
        self.attach_local_control(id).await
    }

    /// Spawn the in-process control client, enumerate the session, start the
    /// pumps.
    ///
    /// The caller serialises attaches (`tmux_attach`); this is not reentrant.
    /// A session that has no control link yet (one whose rows were converted by
    /// `state migrate`) gets it here: attaching IS what creates it.
    async fn attach_local_control(&self, id: SessionId) -> Result<Arc<TmuxControl>> {
        // Row first: `sync_window` records every pane against it.
        self.ensure_control_link(id).await?;
        let target = self.tmux_target(id).await?;
        let prefix = crate::tmux::host_prefix(&target.host);
        let control = Arc::new(TmuxControl::spawn(&prefix, &target.socket, &target.name)?);

        // Subscribe BEFORE enumerating: `%output` is not replayed, so a pane
        // added between the enumeration and the subscription would stream into
        // nothing until it next prints.
        let events = control.subscribe();

        let view = match TmuxView::enumerate(&control, &target.name).await {
            Ok(view) => view,
            Err(e) => {
                // Do not leave a client attached to a session we could not adopt.
                let _ = control.close().await;
                // The client dying during the handshake means one of two things
                // — the session is gone, or the host never answered — and
                // `attach` cannot tell them apart (only a one-shot's exit 255
                // can, arch.md §6). Say what happened instead of surfacing the
                // raw "closed".
                return Err(Error::Sink(format!(
                    "could not attach to tmux session `{}` on {}: {e}",
                    target.name,
                    crate::tmux::host_label(&target.host)
                )));
            }
        };

        if let Err(e) = self.reconcile_all(id, &view, Some(&control)).await {
            let _ = control.close().await;
            return Err(e);
        }

        self.tmux_controls
            .lock()
            .await
            .insert(id, Arc::clone(&control));
        self.record_control(id, &control).await;
        self.start_event_pump(id, Arc::clone(&control), events);
        Ok(control)
    }

    /// Attach in daemon mode: one `OpenTmux` (create-or-reuse the client) that
    /// answers with the structure, so the rows come from the same round trip
    /// that opened the session.
    async fn attach_daemon(&self, id: SessionId) -> Result<()> {
        // Row first: `sync_window` records every pane against it.
        self.ensure_control_link(id).await?;
        let target = self.tmux_target(id).await?;
        let Some(client) = &self.daemon else {
            return Err(Error::Daemon("no daemon configured".into()));
        };
        let structure = client
            .open_tmux(
                id,
                target.host,
                target.socket,
                &target.name,
                target.cols,
                target.rows,
            )
            .await?;
        let view = TmuxView::from_structure(structure);
        self.reconcile_all(id, &view, None).await?;
        Ok(())
    }

    /// Enumerate a tmux session through a client this process holds.
    async fn view_with(&self, control: &Arc<TmuxControl>, id: SessionId) -> Result<TmuxView> {
        let target = self.tmux_target(id).await?;
        TmuxView::enumerate(control, &target.name).await
    }

    /// Note the live client on the session's control link (ADR 0010 §4: the
    /// link IS the connection, so the handle belongs on the link row as well as
    /// in the session-keyed map). Best effort: a session that has no control
    /// link yet (attachment happens before the row exists on the very first
    /// attach) simply has nothing to record.
    async fn record_control(&self, id: SessionId, control: &Arc<TmuxControl>) {
        let Some(link_id) = self.control_link_id(id).await else {
            return;
        };
        let Some(existing) = self.links.get(link_id).await else {
            return;
        };
        let rebuilt = Link {
            id: link_id,
            info: existing.info.clone(),
            transport: Arc::clone(&existing.transport),
            control: Some(Arc::clone(control)),
            connected: true,
        };
        self.links.insert(rebuilt).await;
    }

    /// The session's one link (ADR 0010 §4, "One tmux session, one link").
    async fn control_link_id(&self, id: SessionId) -> Option<LinkId> {
        self.links
            .list()
            .await
            .into_iter()
            .find(|l| l.info.session_id == id && l.info.is_control())
            .map(|l| l.id)
    }

    async fn require_free_session_name(&self, name: &str) -> Result<()> {
        if name.trim().is_empty() {
            return Err(Error::Invalid("session name must not be empty".into()));
        }
        if self.sessions.get_by_name(name).await.is_some() {
            return Err(Error::SessionExists(name.to_string()));
        }
        Ok(())
    }

    /// Shared tail of create/adopt: session row, control link, attach, save.
    async fn finish_tmux_session(
        &self,
        name: &str,
        host: TmuxHostConfig,
        socket: TmuxSocket,
        created: bool,
    ) -> Result<SessionId> {
        let sid = self
            .add_session(SessionInfo::new_tmux(
                name,
                host.clone(),
                socket.clone(),
                name,
            ))
            .await?;

        let outcome = async {
            self.open_control_link(sid).await?;
            self.save_state().await
        }
        .await;
        if let Err(e) = outcome {
            // Rows first, so nothing points at a session that is about to go.
            // An adopted session is left running: the user's tmux was there
            // before us, and a failed *adoption* must not destroy it.
            let _ = self.detach_session(sid).await;
            if created {
                let _ = self
                    .run_tmux(
                        &host,
                        &socket,
                        &[
                            "kill-session".to_string(),
                            "-t".to_string(),
                            name.to_string(),
                        ],
                    )
                    .await;
            }
            return Err(e);
        }
        Ok(sid)
    }

    /// Create the session's single link and attach its control client — in the
    /// daemon when there is one, here otherwise.
    async fn open_control_link(&self, id: SessionId) -> Result<LinkId> {
        let link_id = self.ensure_control_link(id).await?;
        match &self.daemon {
            Some(_) => self.attach_daemon(id).await?,
            None => {
                self.ensure_local_control(id).await?;
            }
        }
        Ok(link_id)
    }

    /// Make sure the session has its control link **row**, and return its id.
    ///
    /// Idempotent: this is the one place that materialises the link, so a
    /// session created by `session new --tmux`, one adopted, and one whose rows
    /// came from `state migrate` all end up with exactly one. Attaching is a
    /// separate step (`open_control_link` / `ensure_local_control` /
    /// `attach_daemon`): a row is metadata, a client is a process.
    async fn ensure_control_link(&self, id: SessionId) -> Result<LinkId> {
        if let Some(link_id) = self.control_link_id(id).await {
            return Ok(link_id);
        }
        let info = self.get_session(id).await?;
        let SessionBackend::Tmux {
            host,
            socket,
            session,
        } = info.backend
        else {
            return Err(Error::NotTmuxSession(id));
        };
        let link_id = LinkId::new();
        // The link is the connection to the *server*, so the process is the
        // connection: `address` records how the tmux protocol is carried (a
        // local `tmux` process, or an `ssh` invocation), while the live client
        // is the `TmuxControl` handle in `control`. Neither a transport nor a
        // sink is ever used to move these bytes — the endpoint says so.
        let prefix = crate::tmux::host_prefix(&host);
        let argv = crate::tmux::argv(&prefix, &socket, &session);
        let address = control_link_address(&host, &argv[prefix.len()..]);

        let link = Link {
            id: link_id,
            info: crate::model::LinkInfo {
                id: link_id,
                session_id: id,
                name: format!("{session}-ctl"),
                created_at: chrono::Utc::now(),
                address,
                endpoint: EndpointRef::Control,
            },
            transport: Arc::new(NoopTransport),
            control: None,
            connected: true,
        };
        self.links.insert(link).await;
        Ok(link_id)
    }

    // ----- enumeration -----

    /// Where a tmux session lives, plus the size the caller saw.
    ///
    /// The daemon needs this for `OpenTmux` (it never reads `state.json`); the
    /// size is informational there (`-C attach` takes none).
    async fn tmux_target(&self, id: SessionId) -> Result<TmuxTarget> {
        let info = self.get_session(id).await?;
        let SessionBackend::Tmux {
            host,
            socket,
            session,
        } = info.backend
        else {
            return Err(Error::NotTmuxSession(id));
        };
        let size = self
            .windows
            .list_all()
            .await
            .iter()
            .find(|w| w.info.session_id == id)
            .map(|w| (w.info.cols, w.info.rows))
            .unwrap_or((80, 24));
        Ok(TmuxTarget {
            host,
            socket,
            name: session,
            cols: size.0,
            rows: size.1,
        })
    }

    /// Re-read every window and pane of the session from one enumeration.
    ///
    /// Idempotent: rows are matched by handle (`@N`/`%N`, never the index), so
    /// running it twice changes nothing. Rows tmux no longer has are dropped.
    /// `local` is the in-process client (daemon mode passes `None`: the daemon
    /// already owns the subscribers, and rows only mirror them).
    async fn reconcile_all(
        &self,
        id: SessionId,
        view: &TmuxView,
        local: Option<&Arc<TmuxControl>>,
    ) -> Result<bool> {
        let _guard = self.tmux_reconcile.lock().await;

        let mut changed = false;
        let mut seen: HashSet<String> = HashSet::new();
        for window in &view.windows {
            seen.insert(window.handle.clone());
            changed |= self.sync_window(id, view, window, local).await?;
        }

        let stale: Vec<WindowId> = self
            .windows
            .list_all()
            .await
            .iter()
            .filter(|w| w.info.session_id == id)
            .filter(|w| match w.info.backend.tmux_handle() {
                Some(handle) => !seen.contains(handle),
                None => false,
            })
            .map(|w| w.info.id)
            .collect();
        for wid in stale {
            self.forget_window_row(wid).await;
            changed = true;
        }

        if changed {
            self.rebuild_session_manager(id).await?;
            self.save_state().await?;
        }
        Ok(changed)
    }

    /// Re-read one window (an event's answer to "what changed?").
    async fn reconcile_window(
        &self,
        id: SessionId,
        view: &TmuxView,
        handle: &str,
        local: Option<&Arc<TmuxControl>>,
    ) -> Result<bool> {
        let _guard = self.tmux_reconcile.lock().await;

        let Some(window) = view.windows.iter().find(|w| w.handle == handle) else {
            // Gone between the notification and the query: treat it as the
            // close it already is.
            let changed = self.forget_window_by_handle(id, handle).await;
            if changed {
                self.rebuild_session_manager(id).await?;
                self.save_state().await?;
            }
            return Ok(changed);
        };

        let changed = self.sync_window(id, view, window, local).await?;
        if changed {
            self.rebuild_session_manager(id).await?;
            self.save_state().await?;
        }
        Ok(changed)
    }

    /// Enumerate through this process's client, then reconcile one window
    /// (the event pump's path — daemon mode has no event pump here).
    async fn reconcile_window_from_control(
        &self,
        id: SessionId,
        control: &Arc<TmuxControl>,
        handle: &str,
    ) -> Result<bool> {
        let view = self.view_with(control, id).await?;
        self.reconcile_window(id, &view, handle, Some(control))
            .await
    }

    /// Upsert one window and its panes; start a subscriber for each pane that
    /// did not exist before.
    ///
    /// The caller holds `tmux_reconcile`.
    async fn sync_window(
        &self,
        id: SessionId,
        view: &TmuxView,
        window: &WindowLine,
        local: Option<&Arc<TmuxControl>>,
    ) -> Result<bool> {
        let Some(link_id) = self.control_link_id(id).await else {
            return Err(Error::Sink(format!(
                "session {id} is tmux-backed but has no control link"
            )));
        };
        let mut changed = false;

        let wid = match self.window_for_handle_in(id, &window.handle).await {
            Some(wid) => {
                let Some(arc) = self.windows.get(wid).await else {
                    return Ok(false);
                };
                let mut updated = arc.info.clone();
                let before = updated.clone();
                updated.name = window.name.clone();
                updated.cols = window.cols;
                updated.rows = window.rows;
                if updated != before {
                    let w = Window::new(updated);
                    *w.panes.lock().await = arc.panes.lock().await.clone();
                    self.windows.insert(w).await;
                    changed = true;
                }
                wid
            }
            None => {
                let wid = WindowId::new();
                let info = WindowInfo {
                    id: wid,
                    session_id: id,
                    name: window.name.clone(),
                    created_at: chrono::Utc::now(),
                    cols: window.cols,
                    rows: window.rows,
                    backend: WindowBackend::Tmux {
                        handle: window.handle.clone(),
                    },
                };
                self.windows.insert(Window::new(info)).await;
                changed = true;
                wid
            }
        };

        let empty: Vec<PaneLine> = Vec::new();
        let panes = view.panes.get(&window.handle).unwrap_or(&empty);

        let mut seen: HashSet<String> = HashSet::new();
        for pane in panes {
            seen.insert(pane.handle.clone());
            match self.pane_for_tmux_handle(id, &pane.handle).await {
                Some(existing) => {
                    let mut updated = existing.clone();
                    let before = updated.clone();
                    updated.cols = pane.cols;
                    updated.rows = pane.rows;
                    updated.x = pane.x;
                    updated.y = pane.y;
                    updated.window_id = Some(wid);
                    if updated != before {
                        self.panes.insert(updated).await;
                        changed = true;
                    }
                    if let Some(arc) = self.windows.get(wid).await {
                        let mut list = arc.panes.lock().await;
                        if !list.contains(&existing.id) {
                            list.push(existing.id);
                            changed = true;
                        }
                    }
                }
                None => {
                    let pid = PaneId::new();
                    let buffer_id = self
                        .create_buffer(
                            id,
                            format!("tmux-{}", pane.handle),
                            BufferOrigin::Tmux {
                                link_id,
                                pane_handle: pane.handle.clone(),
                            },
                        )
                        .await;
                    let info = PaneInfo {
                        id: pid,
                        session_id: id,
                        name: format!("p-{pid}"),
                        created_at: chrono::Utc::now(),
                        cols: pane.cols,
                        rows: pane.rows,
                        x: pane.x,
                        y: pane.y,
                        buffer_id: Some(buffer_id),
                        window_id: Some(wid),
                    };
                    self.panes.insert(info).await;
                    if let Some(arc) = self.windows.get(wid).await {
                        arc.panes.lock().await.push(pid);
                    }
                    // A pane with no subscriber loses its bytes: tmux replays
                    // nothing, so the subscriber starts with the row.
                    self.start_pane_pump(id, local, buffer_id, &pane.handle)
                        .await;
                    changed = true;
                }
            }
        }

        // Panes this window no longer has.
        let mut stale = Vec::new();
        for pane in self.panes.list_all().await {
            if pane.session_id != id || pane.window_id != Some(wid) {
                continue;
            }
            let Some(buffer_id) = pane.buffer_id else {
                continue;
            };
            let Some(buffer) = self.buffers.get(buffer_id).await else {
                continue;
            };
            if let BufferOrigin::Tmux { pane_handle, .. } = buffer.origin {
                if !seen.contains(&pane_handle) {
                    stale.push(pane.id);
                }
            }
        }
        for pid in stale {
            self.forget_pane_row(pid).await;
            changed = true;
        }

        Ok(changed)
    }

    async fn window_for_handle_in(&self, id: SessionId, handle: &str) -> Option<WindowId> {
        self.windows
            .list_all()
            .await
            .iter()
            .find(|w| w.info.session_id == id && w.info.backend.tmux_handle() == Some(handle))
            .map(|w| w.info.id)
    }

    async fn pane_for_tmux_handle(&self, id: SessionId, key: &str) -> Option<PaneInfo> {
        for pane in self.panes.list_all().await {
            if pane.session_id != id {
                continue;
            }
            let Some(buffer_id) = pane.buffer_id else {
                continue;
            };
            if let Some(buffer) = self.buffers.get(buffer_id).await {
                if matches!(
                    &buffer.origin,
                    BufferOrigin::Tmux { pane_handle, .. } if pane_handle == key
                ) {
                    return Some(pane);
                }
            }
        }
        None
    }

    /// Drop a window row and the panes that were displayed in it.
    async fn forget_window_row(&self, wid: WindowId) {
        let pane_ids: Vec<PaneId> = match self.windows.get(wid).await {
            Some(window) => window.panes.lock().await.clone(),
            None => Vec::new(),
        };
        for pid in pane_ids {
            self.forget_pane_row(pid).await;
        }
        self.windows.remove(wid).await;
    }

    /// Drop a window by its tmux handle. Returns whether anything was removed.
    async fn forget_window_by_handle(&self, id: SessionId, handle: &str) -> bool {
        match self.window_for_handle_in(id, handle).await {
            Some(wid) => {
                self.forget_window_row(wid).await;
                true
            }
            None => false,
        }
    }

    /// Drop a pane row, its bridge and its buffer. Used when tmux says the pane
    /// is gone, so `close()` only ever releases OUR pump (a control bridge's
    /// `close` is a no-op by design).
    async fn forget_pane_row(&self, pid: PaneId) {
        let buffer_id = self.panes.get(pid).await.and_then(|pane| pane.buffer_id);
        if let Some(buffer_id) = buffer_id {
            let bridge = self.buffer_bridges.lock().await.remove(&buffer_id);
            if let Some(bridge) = bridge {
                let _ = bridge.close().await;
            }
            self.buffer_memory.lock().await.remove(&buffer_id);
            self.buffers.remove(buffer_id).await;
            if let Some(client) = &self.daemon {
                let _ = client.delete_buffer(buffer_id).await;
            }
        }
        self.panes.remove(pid).await;
    }

    // ----- pumps -----

    /// One task per session: structure changes become registry rows.
    ///
    /// In-process mode only. In daemon mode the client is the daemon's, so
    /// there is nothing here to subscribe: structure is enumerated on touch
    /// (arch.md §7) and `%exit` only flips the daemon slot's `alive` flag.
    fn start_event_pump(
        &self,
        id: SessionId,
        control: Arc<TmuxControl>,
        mut events: tokio::sync::mpsc::UnboundedReceiver<TmuxEvent>,
    ) {
        let manager = self.clone();
        tokio::spawn(async move {
            // Ends when the channel closes — the reader thread drops every
            // sender on EOF, `close()` and `%exit`.
            while let Some(event) = events.recv().await {
                let action = manager
                    .session_managers
                    .lock()
                    .await
                    .get(&id)
                    .and_then(|backend| backend.accept_tmux_event(&event));
                match action {
                    // A rename and a layout change are the same question as an
                    // add: "what does this window look like now?"
                    Some(TmuxEventAction::RefreshWindow(handle)) => {
                        if let Err(e) = manager
                            .reconcile_window_from_control(id, &control, &handle)
                            .await
                        {
                            tracing::warn!("tmux: could not reconcile window {handle}: {e}");
                        }
                    }
                    Some(TmuxEventAction::RemoveWindow(handle)) => {
                        if manager.forget_window_by_handle(id, &handle).await {
                            if let Err(e) = manager.rebuild_session_manager(id).await {
                                tracing::warn!("tmux: could not rebuild session manager: {e}");
                            }
                            if let Err(e) = manager.save_state().await {
                                tracing::warn!(
                                    "tmux: could not save state after {handle} closed: {e}"
                                );
                            }
                        }
                    }
                    // Nothing in xscore stores "the current window" yet, so
                    // these are informational (arch.md §7 lists them as state
                    // to update when a renderer exists).
                    Some(TmuxEventAction::Ignore) => {}
                    Some(TmuxEventAction::Exit(reason)) => {
                        // The client is the session's single connection: with
                        // it gone the link is disconnected (which
                        // `link_is_connected` reports from the closed handle)
                        // and every pane pump ends, because `%exit` shuts the
                        // subscriber channels down.
                        tracing::info!("tmux: control client for session {id} exited: {reason}");
                        break;
                    }
                    None => {
                        tracing::warn!("tmux: session {id} has no backend SessionManager");
                        break;
                    }
                }
            }
        });
    }

    /// Give a new pane its `%output %N` subscriber, wherever the client lives.
    ///
    /// In-process: a task here appending to `buffer_memory`. Daemon: one
    /// `Attach`, and the daemon's own pump fills the daemon's buffer — which is
    /// the whole point of moving the client there.
    async fn start_pane_pump(
        &self,
        id: SessionId,
        local: Option<&Arc<TmuxControl>>,
        buffer_id: BufferId,
        key: &str,
    ) {
        if let Some(control) = local {
            self.start_output_pump(Arc::clone(control), buffer_id, key.to_string())
                .await;
            return;
        }
        if let Some(client) = &self.daemon {
            if let Err(e) = client.attach_control_buffer(id, buffer_id, key).await {
                // Not fatal: the row is still right, and the next data op
                // re-attaches (`control_bridge`).
                tracing::warn!("tmux: daemon could not subscribe pane {key}: {e}");
            }
        }
    }

    /// One task per pane: `%output %N` appended to that pane's buffer.
    async fn start_output_pump(&self, control: Arc<TmuxControl>, buffer_id: BufferId, key: String) {
        let buffer = self.buffer_for(buffer_id).await;
        let mut rx = match control.output(&key).await {
            Ok(rx) => rx,
            Err(e) => {
                tracing::warn!("tmux: could not subscribe to pane {key}: {e}");
                return;
            }
        };
        tokio::spawn(async move {
            while let Some(bytes) = rx.recv().await {
                buffer.lock().await.push_slice(&bytes);
            }
        });
    }

    /// Run one command on the session's control client, wherever it lives.
    ///
    /// Only the mutations that must stay on the one client use this
    /// (`new-window`, `split-window`, `kill-pane`). Data goes through the
    /// pane's bridge and structure through `Structure`; neither is an exec.
    async fn tmux_send(&self, id: SessionId, command: &str) -> Result<Vec<String>> {
        match &self.daemon {
            Some(client) => client.tmux_command(id, command).await,
            None => self.ensure_local_control(id).await?.send(command).await,
        }
    }

    /// Re-read one window after a mutation and reconcile it into rows.
    async fn refresh_window(&self, id: SessionId, handle: &str) -> Result<bool> {
        match &self.daemon {
            Some(_) => {
                let view = self.tmux_view(id).await?;
                self.reconcile_window(id, &view, handle, None).await
            }
            None => {
                let control = self.ensure_local_control(id).await?;
                self.reconcile_window_from_control(id, &control, handle)
                    .await
            }
        }
    }

    /// One enumeration through whichever client holds the session.
    async fn tmux_view(&self, id: SessionId) -> Result<TmuxView> {
        match &self.daemon {
            Some(client) => {
                let structure = match client.structure(id).await {
                    Ok(structure) => structure,
                    // No client in the daemon yet (it restarted): opening one
                    // enumerates too.
                    Err(_) => {
                        let target = self.tmux_target(id).await?;
                        client
                            .open_tmux(
                                id,
                                target.host,
                                target.socket,
                                &target.name,
                                target.cols,
                                target.rows,
                            )
                            .await?
                    }
                };
                Ok(TmuxView::from_structure(structure))
            }
            None => {
                let control = self.ensure_local_control(id).await?;
                self.view_with(&control, id).await
            }
        }
    }

    // ----- one-shot tmux commands (arch.md §6, "two exec shapes") -----

    async fn tmux_session_exists(
        &self,
        host: &TmuxHostConfig,
        socket: &TmuxSocket,
        name: &str,
    ) -> Result<bool> {
        let out = self
            .run_tmux(
                host,
                socket,
                &[
                    "has-session".to_string(),
                    "-t".to_string(),
                    name.to_string(),
                ],
            )
            .await?;
        if out.status.success() {
            return Ok(true);
        }
        if out.status.code() == Some(255) {
            return Err(self.tmux_failure(host, name, "has-session", &out));
        }
        // Exit 1 is "no such session": the answer, not a failure.
        Ok(false)
    }

    /// Run one `tmux` command and return its output (the other exec shape: no
    /// state to keep, so no client).
    async fn run_tmux(
        &self,
        host: &TmuxHostConfig,
        socket: &TmuxSocket,
        args: &[String],
    ) -> Result<std::process::Output> {
        let argv = crate::tmux::one_shot_argv(host, socket, args);
        let (program, rest) = argv
            .split_first()
            .ok_or_else(|| Error::Sink("empty tmux command line".into()))?;
        tokio::process::Command::new(program)
            .args(rest)
            .output()
            .await
            .map_err(|e| Error::TmuxUnreachable {
                host: crate::tmux::host_label(host),
                reason: format!("spawn `{program}`: {e}"),
            })
    }

    /// Turn a failed one-shot into the error the failure semantics table names
    /// (arch.md §6): exit 255 means the host never answered, everything else
    /// means tmux answered and refused.
    fn tmux_failure(
        &self,
        host: &TmuxHostConfig,
        session: &str,
        what: &str,
        out: &std::process::Output,
    ) -> Error {
        let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
        if out.status.code() == Some(255) {
            return Error::TmuxUnreachable {
                host: crate::tmux::host_label(host),
                reason: if stderr.is_empty() {
                    "exit 255".into()
                } else {
                    stderr
                },
            };
        }
        Error::Sink(format!(
            "tmux {what} -t {session} failed ({}): {stderr}",
            out.status.code().unwrap_or(-1)
        ))
    }

    // ===== add_link =====

    /// Create a link and give it a usable pane in the resolved existing session.
    /// See `add_link_in` for the three branches.
    pub async fn add_link(
        &self,
        spec: ConnectSpec,
        address: LinkAddress,
        id: Option<LinkId>,
        pane: Option<PaneId>,
    ) -> Result<LinkAddResult> {
        let sid = self.resolve_session(None).await?;
        self.add_link_in(sid, spec, address, id, pane).await
    }

    /// v0.6.0 (ADR 0010): `add_link`, scoped to an explicit session. Every
    /// window/pane it creates carries `session`, and a pane it binds must
    /// already belong to the same session (arch.md §8).
    pub async fn add_link_in(
        &self,
        session: SessionId,
        spec: ConnectSpec,
        address: LinkAddress,
        id: Option<LinkId>,
        pane: Option<PaneId>,
    ) -> Result<LinkAddResult> {
        self.require_session(session).await?;
        // ADR 0010 §4: a tmux session has exactly one link — the control
        // client. A second one would be a second mechanism driving the same
        // server session.
        if let Some(existing) = self.sessions.get(session).await {
            if existing.info.is_tmux() {
                return Err(Error::TmuxSessionHasOneLink(session));
            }
        }
        // --- validate before allocating anything (a rejected call must not
        //     leave a spawned child behind) ---
        if let Some(pid) = pane {
            let info = self.panes.get(pid).await.ok_or(Error::PaneNotFound(pid))?;
            if info.session_id != session {
                // Cross-session binding is never implicit (arch.md §8).
                return Err(Error::NotInSession {
                    session,
                    what: "pane",
                    id: pid.to_string(),
                });
            }
        }

        let link_id = match id {
            Some(supplied) if self.links.get(supplied).await.is_some() => {
                return Err(Error::LinkIdExists(supplied));
            }
            Some(supplied) => supplied,
            None => LinkId::new(),
        };

        // v0.5.0 (ADR 0009): with the daemon enabled, a pipe link's REAL
        // transport is created inside the daemon and the CLI only keeps a
        // handle to it. That is what makes the link outlive this process.
        let daemon = self.daemon.clone();
        let buffer_id = BufferId::new();
        if let Some(client) = &daemon {
            client.ensure().await?;
            client.create_link(link_id, spec.transport.clone()).await?;
            if let Err(error) = client.create_buffer(buffer_id).await {
                let _ = client.delete_link(link_id).await;
                return Err(error);
            }
        }

        let transport_arc: Arc<dyn crate::transport::Transport> = match &daemon {
            Some(_) => Arc::new(crate::daemon::DaemonTransport::new(link_id)),
            None => {
                // The transport IS the byte source of a pipe link, so it is
                // opened once here (the other callers of `transport_for` own
                // their own lifetime): a bridge may then read and write without
                // a second handshake. The deleted sink factory used to do this
                // for the raw case; PR4 moved the one call that mattered here.
                let mut transport =
                    transport_factory::transport_for(spec.transport.clone()).await?;
                transport.open().await?;
                Arc::from(transport)
            }
        };
        self.buffers
            .insert(BufferInfo {
                id: buffer_id,
                session_id: session,
                name: format!("b-{}", &buffer_id.to_string()[..8]),
                created_at: chrono::Utc::now(),
                origin: BufferOrigin::Link { link_id },
            })
            .await;

        // --- branch: decide the window / pane / bridge ---
        type Branch = (
            Option<WindowId>,
            PaneId,
            PaneInfo,
            Option<Arc<dyn PaneBridge>>,
            Option<Window>,
        );
        let (window_id, pane_id, pane_record, bridge, window_record): Branch = match pane {
            // (a) bind an existing pane: no window is created — the pane keeps
            //     whatever window it had.
            Some(pid) => {
                let bridge: Arc<dyn PaneBridge> = match &daemon {
                    Some(client) => Arc::new(DaemonBridge::new(Arc::clone(client), buffer_id)),
                    None => {
                        let buf = self.buffer_for(buffer_id).await;
                        Arc::new(DirectBridge::new_with_buffer(
                            Arc::clone(&transport_arc),
                            buf,
                        ))
                    }
                };
                let mut info = self.panes.get(pid).await.ok_or(Error::PaneNotFound(pid))?;
                info.buffer_id = Some(buffer_id);
                (None, pid, info, Some(bridge), None)
            }

            // (b) a pipe link with no pane: native window + first pane.
            None => {
                let wid = WindowId::new();
                let pid = PaneId::new();
                let w = Window::new(WindowInfo {
                    id: wid,
                    session_id: session,
                    name: format!("w-{}", &wid.to_string()[..8]),
                    created_at: chrono::Utc::now(),
                    cols: 120,
                    rows: 30,
                    backend: WindowBackend::Native,
                });
                let _ = w.add_pane(pid).await;
                let bridge: Arc<dyn PaneBridge> = match &daemon {
                    Some(client) => Arc::new(DaemonBridge::new(Arc::clone(client), buffer_id)),
                    None => {
                        let buf = self.buffer_for(buffer_id).await;
                        Arc::new(DirectBridge::new_with_buffer(
                            Arc::clone(&transport_arc),
                            buf,
                        ))
                    }
                };
                (
                    Some(wid),
                    pid,
                    PaneInfo {
                        id: pid,
                        session_id: session,
                        name: format!("p-{pid}"),
                        created_at: chrono::Utc::now(),
                        cols: 80,
                        rows: 24,
                        x: 0,
                        y: 0,
                        buffer_id: Some(buffer_id),
                        window_id: Some(wid),
                    },
                    Some(bridge),
                    Some(w),
                )
            }
        };

        let link_info = crate::model::LinkInfo {
            id: link_id,
            session_id: session,
            name: if spec.name.is_empty() {
                link_id.to_string()
            } else {
                spec.name.clone()
            },
            created_at: chrono::Utc::now(),
            address: address.clone(),
            // Every link this method creates is a pipe: one byte stream, at
            // most one reader. A `Control` link is created only by
            // `open_control_link` (ADR 0010 §D2).
            endpoint: EndpointRef::Pipe(spec.transport.clone()),
        };

        let link = Link {
            id: link_id,
            info: link_info,
            transport: transport_arc,
            control: None,
            connected: true,
        };
        self.links.insert(link).await;
        if let Some(w) = window_record {
            self.windows.insert(w).await;
        }
        self.panes.insert(pane_record).await;
        if let Some(b) = bridge {
            self.buffer_bridges.lock().await.insert(buffer_id, b);
        }

        // ADR 0009: the binding only becomes LIVE at Attach — the daemon
        // starts the pane's pump here, and keeps it running after this
        // process exits. A failure here must not leave an orphaned
        // transport behind, so tear the daemon link back down.
        if let Some(client) = &daemon {
            if let Err(e) = client.attach_buffer(buffer_id, link_id).await {
                let _ = client.delete_link(link_id).await;
                let _ = client.delete_buffer(buffer_id).await;
                self.buffers.remove(buffer_id).await;
                return Err(e);
            }
        }

        self.rebuild_session_manager(session).await?;
        if let Err(e) = self.save_state().await {
            // A failed save means the CLI exits with an error, so nothing
            // will ever reference this link. Do not leave its transport
            // (and a daemon that can therefore never idle-exit) behind.
            if let Some(client) = &daemon {
                let _ = client.delete_link(link_id).await;
                let _ = client.delete_buffer(buffer_id).await;
            }
            self.buffers.remove(buffer_id).await;
            let _ = self.rebuild_session_manager(session).await;
            return Err(e);
        }
        Ok(LinkAddResult {
            link_id,
            buffer_id,
            window_id,
            pane_id,
        })
    }

    /// Full teardown of a link.
    ///
    /// * pipe link: stop feeding its buffers, close the transport once, and
    ///   remove the link. Buffers and their UI panes remain.
    /// * control link: refused (ADR 0010 §D5) — see `del_session` /
    ///   `detach_session` for the two verbs that do tear a tmux session down.
    pub async fn del_link(&self, id: LinkId) -> Result<()> {
        let arc_link = self.links.get(id).await.ok_or(Error::LinkNotFound(id))?;

        // ADR 0010 §D5: every pane in a tmux session routes through its control
        // link, so deleting it would leave them with buffers and no way back.
        if arc_link.info.is_control() {
            return Err(Error::ControlLinkNotDeletable(id));
        }

        let linked_buffers: Vec<BufferId> = self
            .buffers
            .list_all()
            .await
            .into_iter()
            .filter_map(|buffer| match buffer.origin {
                BufferOrigin::Link { link_id } if link_id == id => Some(buffer.id),
                _ => None,
            })
            .collect();
        for buffer_id in linked_buffers {
            let bridge = self.buffer_bridges.lock().await.remove(&buffer_id);
            if let Some(b) = bridge {
                // DirectBridge::close() stops the pump only (in daemon
                // mode: Detach), so the transport below is closed
                // exactly once, on purpose.
                let _ = b.close().await;
            }
        }
        match &self.daemon {
            // ADR 0009: the daemon owns the transport. DeleteLink
            // closes it once and unbinds its panes — but keeps their
            // slots and buffers, so the history is still readable and
            // a later re-attach resumes where it left off.
            Some(client) => {
                // Best effort: if the daemon is down the transport is
                // already gone, which is the state we wanted anyway.
                let _ = client.delete_link(id).await;
            }
            None => {
                arc_link.transport.close().await?;
            }
        }

        self.links.remove(id).await;
        self.rebuild_session_manager(arc_link.info.session_id)
            .await?;
        self.save_state().await?;
        Ok(())
    }

    // ===== add_window / del_window =====

    /// Create a window in the resolved existing session. See `add_window_in`.
    pub async fn add_window(&self, name: Option<String>, cols: u16, rows: u16) -> Result<WindowId> {
        let sid = self.resolve_session(None).await?;
        self.add_window_in(sid, name, cols, rows).await
    }

    /// Create a window in `session`.
    ///
    /// A tmux-backed session gets a real window (`new-window` through its
    /// control client; tmux reports the `@N`, the name and the geometry). A
    /// native session gets a purely xscore-side window with the requested
    /// geometry — there is no renderer yet, so it displays nothing (arch.md
    /// §13).
    pub async fn add_window_in(
        &self,
        session: SessionId,
        name: Option<String>,
        cols: u16,
        rows: u16,
    ) -> Result<WindowId> {
        self.require_session(session).await?;
        match self.get_session(session).await?.backend {
            SessionBackend::Native => {
                let window_id = WindowId::new();
                let info = WindowInfo {
                    id: window_id,
                    session_id: session,
                    name: name.unwrap_or_else(|| format!("w-{}", &window_id.to_string()[..8])),
                    created_at: chrono::Utc::now(),
                    cols,
                    rows,
                    backend: WindowBackend::Native,
                };
                self.windows.insert(Window::new(info)).await;
                self.save_state().await?;
                Ok(window_id)
            }
            SessionBackend::Tmux { .. } => {
                let lines = self
                    .tmux_send(session, &crate::tmux::new_window_command(name.as_deref()))
                    .await?;
                let handle = tmux_handle_from(&lines, '@').ok_or_else(|| {
                    Error::Sink(format!(
                        "tmux new-window did not report a window handle: {lines:?}"
                    ))
                })?;
                // Read the new window back: the row must be tmux's view of it,
                // not our guess at its geometry or name.
                self.refresh_window(session, &handle).await?;
                self.window_for_handle_in(session, &handle)
                    .await
                    .ok_or_else(|| {
                        Error::Sink(format!(
                            "tmux created window {handle} but it did not become a row"
                        ))
                    })
            }
        }
    }

    /// Tear down a window row. Panes inside survive as orphans.
    ///
    /// The row is xscore's, not the backend's: for a tmux window the next
    /// reconciliation (any listing, or an event) brings it back, which is why
    /// the verb is about the row only.
    pub async fn del_window(&self, id: WindowId) -> Result<()> {
        let arc_window = self
            .windows
            .get(id)
            .await
            .ok_or(Error::WindowNotFound(id))?;
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

    // ===== add_pane / del_pane =====

    /// Create a pane in the resolved existing session. See `add_pane_in`.
    pub async fn add_pane(
        &self,
        name: Option<String>,
        cols: u16,
        rows: u16,
        x: u16,
        y: u16,
        window: Option<WindowId>,
    ) -> Result<PaneId> {
        let sid = self.resolve_session(None).await?;
        self.add_pane_in(sid, name, cols, rows, x, y, window).await
    }

    /// Create a pane.
    ///
    /// * `window: None` → hollow orphan pane (no link, no window).
    /// * `window: Some(wid)` mirroring a tmux window → split inside that real
    ///   window through the session's control client; the row is the one
    ///   reconciliation creates, whose Buffer origin retains the `%N` on the
    ///   session's existing link (no link is created — a tmux session has
    ///   exactly one).
    /// * `window: Some(wid)` native → hollow pane inside the window.
    ///
    /// v0.6.0 (ADR 0010): the pane belongs to `session`, and the window it is
    /// put into must belong there too (arch.md §8).
    // The argument list is the model's (geometry + window + session); grouping
    // it into a struct would just move the same fields somewhere else.
    #[allow(clippy::too_many_arguments)]
    pub async fn add_pane_in(
        &self,
        session: SessionId,
        name: Option<String>,
        cols: u16,
        rows: u16,
        x: u16,
        y: u16,
        window: Option<WindowId>,
    ) -> Result<PaneId> {
        self.require_session(session).await?;

        // In a tmux session, a pane with no window would be a row no byte could
        // ever reach: `link add` and `pane attach` are both refused there. So
        // "no window" means "the one tmux is showing", exactly like
        // `split-window` with no target.
        let window = match window {
            Some(w) => Some(w),
            None => {
                if let SessionBackend::Tmux { session: name, .. } =
                    self.get_session(session).await?.backend
                {
                    let lines = self
                        .tmux_send(
                            session,
                            &format!("display-message -p -t {name} '#{{window_id}}'"),
                        )
                        .await?;
                    let handle = tmux_handle_from(&lines, '@').ok_or_else(|| {
                        Error::Sink(format!("tmux did not report the current window: {lines:?}"))
                    })?;
                    let wid = self
                        .windows
                        .list_all()
                        .await
                        .into_iter()
                        .find(|w| {
                            w.info.session_id == session
                                && w.info.backend.tmux_handle() == Some(handle.as_str())
                        })
                        .map(|w| w.info.id)
                        .ok_or_else(|| Error::Sink(format!("tmux window {handle} is not a row")))?;
                    Some(wid)
                } else {
                    None
                }
            }
        };

        // A tmux window's panes are tmux's: split it and adopt what tmux
        // creates, rather than inventing a hollow row that no byte could reach.
        if let Some(wid) = window {
            let arc_window = self
                .windows
                .get(wid)
                .await
                .ok_or(Error::WindowNotFound(wid))?;
            if arc_window.info.session_id != session {
                return Err(Error::NotInSession {
                    session,
                    what: "window",
                    id: wid.to_string(),
                });
            }
            if let Some(handle) = arc_window.info.backend.tmux_handle().map(str::to_string) {
                let lines = self
                    .tmux_send(session, &crate::tmux::split_window_command(&handle))
                    .await?;
                let key = tmux_handle_from(&lines, '%').ok_or_else(|| {
                    Error::Sink(format!(
                        "tmux split-window did not report a pane handle: {lines:?}"
                    ))
                })?;
                self.refresh_window(session, &handle).await?;
                let existing = self
                    .pane_for_tmux_handle(session, &key)
                    .await
                    .ok_or_else(|| {
                        Error::Sink(format!(
                            "tmux created pane {key} but it did not become a row"
                        ))
                    })?;
                if let Some(requested) = name {
                    let mut renamed = existing.clone();
                    renamed.name = requested;
                    self.panes.insert(renamed).await;
                    self.save_state().await?;
                }
                return Ok(existing.id);
            }
        }

        let pane_id = PaneId::new();
        let info = PaneInfo {
            id: pane_id,
            session_id: session,
            name: name.unwrap_or_else(|| format!("p-{pane_id}")),
            created_at: chrono::Utc::now(),
            cols,
            rows,
            x,
            y,
            buffer_id: None,
            window_id: window,
        };
        self.panes.insert(info).await;
        if let Some(wid) = window {
            if let Some(arc_window) = self.windows.get(wid).await {
                arc_window.panes.lock().await.push(pane_id);
            }
        }
        self.save_state().await?;
        Ok(pane_id)
    }

    pub async fn del_pane(&self, id: PaneId) -> Result<()> {
        let info = self.panes.get(id).await.ok_or(Error::PaneNotFound(id))?;
        if let Some(wid) = info.window_id {
            if let Some(arc_window) = self.windows.get(wid).await {
                arc_window.panes.lock().await.retain(|x| *x != id);
            }
        }
        self.panes.remove(id).await;
        self.save_state().await?;
        Ok(())
    }

    pub async fn del_buffer(&self, id: BufferId) -> Result<()> {
        let info = self
            .buffers
            .get(id)
            .await
            .ok_or_else(|| Error::Invalid(format!("buffer not found: {id}")))?;
        let bridge = self.buffer_bridges.lock().await.remove(&id);
        if let Some(bridge) = bridge {
            let _ = bridge.close().await;
        }
        self.buffer_memory.lock().await.remove(&id);
        self.buffers.remove(id).await;
        if let Some(client) = &self.daemon {
            let _ = client.delete_buffer(id).await;
        }
        for mut pane in self.panes.list_all().await {
            if pane.buffer_id == Some(id) {
                pane.buffer_id = None;
                self.panes.insert(pane).await;
            }
        }
        self.rebuild_session_manager(info.session_id).await?;
        self.save_state().await?;
        Ok(())
    }

    // ===== pane UI to buffer association =====

    /// Point a UI pane at the buffer produced by a raw link.
    pub async fn attach_pane(&self, pid: PaneId, lid: LinkId) -> Result<()> {
        let link = self.links.get(lid).await.ok_or(Error::LinkNotFound(lid))?;
        let pane = self.panes.get(pid).await.ok_or(Error::PaneNotFound(pid))?;
        if pane.session_id != link.info.session_id {
            return Err(Error::NotInSession {
                session: link.info.session_id,
                what: "pane",
                id: pid.to_string(),
            });
        }
        if link.info.is_control() {
            return Err(Error::Invalid(format!(
                "link {lid} is a tmux control link; pane {pid} must reference the buffer \
                 created for a tmux pane handle"
            )));
        }
        let buffer_id = self
            .session_managers
            .lock()
            .await
            .get(&pane.session_id)
            .and_then(|manager| manager.buffer_for_link(lid))
            .ok_or_else(|| Error::Invalid(format!("link {lid} produced no buffer")))?;
        let buffer = self
            .buffers
            .get(buffer_id)
            .await
            .ok_or_else(|| Error::Invalid(format!("buffer {buffer_id} not found")))?;

        if let Some(client) = &self.daemon {
            if client.attach_buffer(buffer.id, lid).await.is_err() {
                client.ensure().await?;
                client.create_link(lid, pipe_transport(&link.info)?).await?;
                client.create_buffer(buffer.id).await?;
                client.attach_buffer(buffer.id, lid).await?;
            }
            let bridge: Arc<dyn PaneBridge> =
                Arc::new(DaemonBridge::new(Arc::clone(client), buffer.id));
            self.buffer_bridges.lock().await.insert(buffer.id, bridge);
        }

        let mut updated = pane;
        updated.buffer_id = Some(buffer.id);
        self.panes.insert(updated).await;
        self.save_state().await?;
        Ok(())
    }

    /// Remove the UI association. The buffer and backend pump keep running.
    pub async fn detach_pane(&self, pid: PaneId) -> Result<()> {
        let mut pane = self.panes.get(pid).await.ok_or(Error::PaneNotFound(pid))?;
        if pane.buffer_id.take().is_none() {
            return Err(Error::NotAttached(pid));
        }
        self.panes.insert(pane).await;
        self.save_state().await?;
        Ok(())
    }

    // ===== window ⇄ pane association =====

    pub async fn add_pane_to_window(&self, wid: WindowId, pid: PaneId) -> Result<()> {
        let arc_window = self
            .windows
            .get(wid)
            .await
            .ok_or(Error::WindowNotFound(wid))?;
        let pane_info = self.panes.get(pid).await.ok_or(Error::PaneNotFound(pid))?;
        // v0.6.0 (ADR 0010 §8): a window and its panes share one session.
        if arc_window.info.session_id != pane_info.session_id {
            return Err(Error::NotInSession {
                session: arc_window.info.session_id,
                what: "pane",
                id: pid.to_string(),
            });
        }
        {
            let panes = arc_window.panes.lock().await;
            if panes.contains(&pid) {
                return Err(Error::AlreadyInWindow {
                    window: wid,
                    pane: pid,
                });
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
        let arc_window = self
            .windows
            .get(wid)
            .await
            .ok_or(Error::WindowNotFound(wid))?;
        let _ = self.panes.get(pid).await.ok_or(Error::PaneNotFound(pid))?;
        {
            let mut panes = arc_window.panes.lock().await;
            if !panes.contains(&pid) {
                return Err(Error::NotInWindow {
                    window: wid,
                    pane: pid,
                });
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
        let target = self
            .windows
            .get(wid)
            .await
            .ok_or(Error::WindowNotFound(wid))?;
        // Guard BEFORE touching the old window: a rejected move must leave the
        // pane where it was, not half-moved (arch.md §8 — cross-session moves
        // are never an implicit side effect).
        if target.info.session_id != info.session_id {
            return Err(Error::NotInSession {
                session: target.info.session_id,
                what: "pane",
                id: pid.to_string(),
            });
        }
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
        let info = self.panes.get(pid).await.ok_or(Error::PaneNotFound(pid))?;
        // Resize the real thing first. If the transport has no size (serial) or
        // the call fails, nothing has changed yet — a failed resize must not
        // leave a modified `PaneInfo` behind, which is the same silent lie as
        // returning Ok without doing anything.
        if let Some(buffer_id) = info.buffer_id {
            if let Some(bridge) = self.buffer_bridges.lock().await.get(&buffer_id).cloned() {
                bridge.resize(cols, rows).await?;
            }
        }
        let mut updated = info;
        updated.cols = cols;
        updated.rows = rows;
        self.panes.insert(updated).await;
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
        let b = self.bridge_for(pid).await?;
        b.read(buf).await
    }

    pub async fn data_write(&self, pid: PaneId, data: &[u8]) -> Result<usize> {
        let b = self.bridge_for(pid).await?;
        b.write(data).await
    }

    /// Resize a pane's bridge AND its info (cols/rows). Errors with
    /// `NotAttached(pid)` for orphan panes — use `resize_pane` if you
    /// want to update geometry without going through the bridge.
    pub async fn data_resize(&self, pid: PaneId, cols: u16, rows: u16) -> Result<()> {
        let b = self.bridge_for(pid).await?;
        b.resize(cols, rows).await?;
        if let Some(mut info) = self.panes.get(pid).await {
            info.cols = cols;
            info.rows = rows;
            self.panes.insert(info).await;
        }
        self.save_state().await?;
        Ok(())
    }

    /// Resolve a pane's live bridge, distinguishing the two ways it can
    /// be missing:
    ///
    /// * `NotAttached(pid)` — the pane has no link binding at all
    ///   (orphan). The user must `pane attach`.
    /// * `LinkNotConnected(lid)` — the pane IS bound, but the link's
    ///   transport died with the process that owned it (only possible
    ///   for direct links, and only after a restart). The binding is
    ///   real; the connection is not. Re-adding the link reconnects.
    ///
    /// v0.4.1 (P0-3): before this split, both cases surfaced as
    /// `NotAttached`, which told the user their still-bound pane had
    /// never been attached — persisted state and runtime disagreed.
    ///
    /// v0.5.0 (ADR 0009): with the daemon enabled, direct panes get a
    /// `DaemonBridge` **on demand** instead of only right after an
    /// `attach`. That is what makes `data read/write` work from a process
    /// that only loaded `state.json` — the transport and the buffer were
    /// never in this process to begin with.
    async fn bridge_for(&self, pid: PaneId) -> Result<Arc<dyn PaneBridge>> {
        let pane = self.panes.get(pid).await.ok_or(Error::PaneNotFound(pid))?;
        let buffer_id = pane.buffer_id.ok_or(Error::NotAttached(pid))?;
        if let Some(bridge) = self.buffer_bridges.lock().await.get(&buffer_id).cloned() {
            return Ok(bridge);
        }
        let buffer = self
            .buffers
            .get(buffer_id)
            .await
            .ok_or_else(|| Error::Invalid(format!("buffer {buffer_id} not found")))?;
        let bridge: Arc<dyn PaneBridge> = match buffer.origin.clone() {
            BufferOrigin::Tmux {
                link_id,
                pane_handle,
            } => self.control_bridge(&buffer, link_id, &pane_handle).await?,
            BufferOrigin::Link { link_id } => self.raw_bridge(&buffer, link_id).await?,
        };
        self.buffer_bridges
            .lock()
            .await
            .insert(buffer_id, Arc::clone(&bridge));
        Ok(bridge)
    }

    /// The bridge for a pane that lives on its session's control link, if it
    /// does.
    ///
    /// Attaching here — rather than only at creation — is what makes
    /// `data read/write` work from a process that did nothing but load
    /// `state.json` (arch.md §7: the structure is live, not a mirror).
    ///
    /// v0.6.0 PR5: with a daemon, the read path is the **daemon's** buffer, so
    /// the daemon must hold the client (it may have restarted) and a subscriber
    /// for this `%N` before the bridge is handed out. Both calls are
    /// idempotent, and the second is what makes a pane discovered late
    /// (arch.md §13: no history) start streaming from now on.
    async fn control_bridge(
        &self,
        info: &BufferInfo,
        link_id: LinkId,
        key: &str,
    ) -> Result<Arc<dyn PaneBridge>> {
        let Some(link) = self.links.get(link_id).await else {
            return Err(Error::LinkNotConnected(link_id));
        };
        if !link.info.is_control() {
            return Err(Error::LinkNotConnected(link_id));
        }
        match &self.daemon {
            Some(client) => {
                if !client.probe_tmux(info.session_id).await {
                    let target = self.tmux_target(info.session_id).await?;
                    client
                        .open_tmux(
                            info.session_id,
                            target.host,
                            target.socket,
                            &target.name,
                            target.cols,
                            target.rows,
                        )
                        .await?;
                }
                client
                    .attach_control_buffer(info.session_id, info.id, key)
                    .await?;
                let bridge: Arc<dyn PaneBridge> =
                    Arc::new(DaemonBridge::new(Arc::clone(client), info.id));
                Ok(bridge)
            }
            None => {
                let control = self.ensure_local_control(info.session_id).await?;
                let buffer = self.buffer_for(info.id).await;
                let bridge: Arc<dyn PaneBridge> =
                    Arc::new(ControlBridge::new(control, key.to_string(), buffer));
                Ok(bridge)
            }
        }
    }

    async fn raw_bridge(&self, info: &BufferInfo, link_id: LinkId) -> Result<Arc<dyn PaneBridge>> {
        if let Some(client) = &self.daemon {
            let link_live = crate::daemon::client::probe_link(link_id)
                .await
                .unwrap_or(false);
            let buffer_live = crate::daemon::client::probe_buffer(info.id)
                .await
                .unwrap_or(false);
            if !link_live && !buffer_live {
                return Err(Error::LinkNotConnected(link_id));
            }
            if link_live {
                client.attach_buffer(info.id, link_id).await?;
            }
            return Ok(Arc::new(DaemonBridge::new(Arc::clone(client), info.id)));
        }
        let link = self
            .links
            .get(link_id)
            .await
            .ok_or(Error::LinkNotConnected(link_id))?;
        if !link.connected {
            return Err(Error::LinkNotConnected(link_id));
        }
        let buffer = self.buffer_for(info.id).await;
        Ok(Arc::new(DirectBridge::new_with_buffer(
            Arc::clone(&link.transport),
            buffer,
        )))
    }

    // ===== internal helpers =====

    /// The buffer, created on first use and kept independently of its UI panes.
    /// detach/re-attach.
    async fn buffer_for(&self, buffer_id: BufferId) -> Arc<Mutex<RingBuffer>> {
        let mut map = self.buffer_memory.lock().await;
        map.entry(buffer_id)
            .or_insert_with(|| Arc::new(Mutex::new(RingBuffer::new(DIRECT_BUFFER_CAPACITY))))
            .clone()
    }

    async fn create_buffer(
        &self,
        session_id: SessionId,
        name: String,
        origin: BufferOrigin,
    ) -> BufferId {
        let id = BufferId::new();
        self.buffers
            .insert(BufferInfo {
                id,
                session_id,
                name,
                created_at: chrono::Utc::now(),
                origin,
            })
            .await;
        id
    }

    async fn buffers_for_session(&self, session_id: SessionId) -> Vec<BufferInfo> {
        self.buffers
            .list_all()
            .await
            .into_iter()
            .filter(|buffer| buffer.session_id == session_id)
            .collect()
    }

    async fn rebuild_session_manager(&self, session_id: SessionId) -> Result<()> {
        let info = self.get_session(session_id).await?;
        let links: Vec<LinkId> = self
            .links
            .list()
            .await
            .into_iter()
            .filter(|link| link.info.session_id == session_id)
            .map(|link| link.id)
            .collect();
        let buffers = self.buffers_for_session(session_id).await;
        if links.is_empty() {
            self.session_managers.lock().await.remove(&session_id);
            return Ok(());
        }
        let manager = SessionManagerRuntime::from_parts(&info, &links, &buffers)?;
        self.session_managers
            .lock()
            .await
            .insert(session_id, manager);
        Ok(())
    }

    // ===== state persistence =====

    pub async fn save_state(&self) -> Result<()> {
        // v0.6.0 (ADR 0010 D8): `sessions` first — the children below all
        // reference one. `list_sessions` sorts by `created_at`, so the file is
        // stable across saves rather than following HashMap iteration order.
        let sessions = self.list_sessions().await?;

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
        let buffers = self.buffers.list_all().await;

        let snapshot = super::state::StateSnapshot {
            schema_version: super::state::SCHEMA_VERSION,
            sessions,
            links,
            buffers,
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
        // v0.6.0 (ADR 0010 D8): sessions BEFORE their children — a link's
        // `session_id` must resolve, and there is deliberately no fallback that
        // invents a session on load.
        for info in snapshot.sessions {
            mgr.sessions.insert(Session::new(info)).await;
        }
        for info in snapshot.buffers {
            if mgr.sessions.get(info.session_id).await.is_some() {
                mgr.buffers.insert(info).await;
            }
        }
        for info in snapshot.links {
            if mgr.sessions.get(info.session_id).await.is_none() {
                tracing::warn!(
                    "hydration: skipping link {} — no session {} in state",
                    info.id,
                    info.session_id
                );
                continue;
            }
            mgr.hydrate_link(info).await;
        }
        for ws in snapshot.windows {
            if mgr.sessions.get(ws.info.session_id).await.is_none() {
                tracing::warn!(
                    "hydration: skipping window {} — no session {} in state",
                    ws.info.id,
                    ws.info.session_id
                );
                continue;
            }
            let w = Window::new(ws.info);
            for pid in ws.panes {
                w.panes.lock().await.push(pid);
            }
            mgr.windows.insert(w).await;
        }
        for pinfo in snapshot.panes {
            if mgr.sessions.get(pinfo.session_id).await.is_none() {
                tracing::warn!(
                    "hydration: skipping pane {} — no session {} in state",
                    pinfo.id,
                    pinfo.session_id
                );
                continue;
            }
            mgr.panes.insert(pinfo).await;
        }
        let session_ids: Vec<SessionId> = mgr
            .sessions
            .list()
            .await
            .into_iter()
            .map(|session| session.info.id)
            .collect();
        for session_id in session_ids {
            if let Err(e) = mgr.rebuild_session_manager(session_id).await {
                tracing::warn!("hydration: session manager for {session_id}: {e}");
            }
        }
        // Nothing else is rebuilt here on purpose. A pipe link's transport died
        // with the process that owned it (see `Link::connected`: false is the
        // honest answer, so data ops say `LinkNotConnected`), and a control
        // link has no client here at all — in daemon mode the daemon holds it
        // (and the buffers), otherwise `ensure_local_control` spawns one on
        // first use. Either way a fresh CLI process works against a session it
        // never created.
        Ok(mgr)
    }

    /// A hydrated link has no live connection yet: the transport of a pipe died
    /// with the previous process, and a control link's client is spawned on
    /// first use. Recording that honestly is what lets data ops report
    /// `LinkNotConnected` instead of the misleading `NotAttached`.
    async fn hydrate_link(&self, info: crate::model::LinkInfo) {
        let link = Link {
            id: info.id,
            info,
            transport: Arc::new(NoopTransport),
            control: None,
            connected: false,
        };
        self.links.insert(link).await;
    }
}

/// Where a tmux session lives, plus the size the caller saw.
///
/// The daemon never reads `state.json` (ADR 0009 §2), so `OpenTmux` has to carry
/// this: host, socket, name. `cols`/`rows` are informational (`-C attach` takes
/// no size).
struct TmuxTarget {
    host: TmuxHostConfig,
    socket: TmuxSocket,
    name: String,
    cols: u16,
    rows: u16,
}

/// One enumeration of a tmux session: its windows, and the panes of each keyed
/// by window handle.
///
/// This is what both modes reconcile from: the daemon answers with the same
/// information the in-process client reads off the wire, so `sync_window` has
/// exactly one shape to consume (arch.md §7, "enumerated on touch").
struct TmuxView {
    windows: Vec<WindowLine>,
    panes: HashMap<String, Vec<PaneLine>>,
}

impl TmuxView {
    /// Build a view from a `Structure` answer (one round trip for the windows,
    /// one per window for the panes — all on the daemon's resident client).
    fn from_structure(structure: crate::daemon::protocol::TmuxStructure) -> Self {
        let mut panes: HashMap<String, Vec<PaneLine>> = HashMap::new();
        for pane in structure.panes {
            panes.entry(pane.window_handle).or_default().push(PaneLine {
                handle: pane.handle,
                cols: pane.cols,
                rows: pane.rows,
                x: pane.x,
                y: pane.y,
            });
        }
        let windows = structure
            .windows
            .into_iter()
            .map(|window| WindowLine {
                handle: window.handle,
                name: window.name,
                cols: window.cols,
                rows: window.rows,
                // Not on the wire: nothing in xscore reads a window's layout or
                // its active flag yet (arch.md §7 lists both as future state).
                layout: String::new(),
                active: false,
            })
            .collect();
        Self { windows, panes }
    }

    /// Enumerate through a client this process holds (`list-windows` once,
    /// `list-panes` per window).
    async fn enumerate(control: &Arc<TmuxControl>, session: &str) -> Result<Self> {
        let lines = control
            .send(&crate::tmux::list_windows_command(session))
            .await?;
        let windows: Vec<WindowLine> = lines
            .iter()
            .filter_map(|line| crate::tmux::parse_window_line(line))
            .collect();

        let mut panes: HashMap<String, Vec<PaneLine>> = HashMap::new();
        for window in &windows {
            let lines = control
                .send(&crate::tmux::list_panes_command(&window.handle))
                .await?;
            let rows: Vec<PaneLine> = lines
                .iter()
                .filter_map(|line| crate::tmux::parse_pane_line(line))
                .collect();
            panes.insert(window.handle.clone(), rows);
        }
        Ok(Self { windows, panes })
    }
}

/// The first line of a tmux command's output that is a handle of the expected
/// kind (`@N` for a window, `%N` for a pane).
///
/// `-P -F '#{window_id}'` makes the create commands answer with their handle,
/// which is the only reliable way to learn it: matching on names would confuse
/// two windows that share one.
fn tmux_handle_from(lines: &[String], sigil: char) -> Option<String> {
    lines
        .iter()
        .map(|line| line.trim())
        .find(|line| line.starts_with(sigil) && line.len() > 1)
        .map(str::to_string)
}

/// The transport spec of a `Pipe` link. A `Control` link has none — its host,
/// socket and session live on the session — so asking one is a caller bug.
fn pipe_transport(info: &crate::model::LinkInfo) -> Result<TransportConfig> {
    match &info.endpoint {
        EndpointRef::Pipe(transport) => Ok(transport.clone()),
        EndpointRef::Control => Err(Error::Invalid(format!(
            "link {} is a control link and has no transport of its own",
            info.id
        ))),
    }
}

/// How a tmux session's control link reaches the server: a local `tmux`
/// process (`LinkAddress::Shell`) or an `ssh` invocation
/// (`LinkAddress::Ssh`). `tmux_argv` is the argv *after* the host prefix.
///
/// Splitting the full argv instead would label every remote session `shell`
/// with `command: "ssh"`, which is how `link show` used to lie about it. The
/// host config is the authority here; the address is its readable projection.
fn control_link_address(host: &TmuxHostConfig, tmux_argv: &[String]) -> LinkAddress {
    match host {
        TmuxHostConfig::Local => {
            let mut words = tmux_argv.iter().cloned();
            let command = words.next().unwrap_or_else(|| "tmux".to_string());
            LinkAddress::Shell {
                command,
                args: words.collect(),
            }
        }
        TmuxHostConfig::Ssh { destination, args } => {
            // `ssh [-p N] [-i key] [destination]`: with no user in the
            // destination ssh uses the local one, and so does this record.
            let (user, host) = match destination.rsplit_once('@') {
                Some((user, host)) => (user.to_string(), host.to_string()),
                None => (
                    std::env::var("USER").unwrap_or_default(),
                    destination.clone(),
                ),
            };
            let mut port: u16 = 22;
            let mut identity = None;
            let mut it = args.iter();
            while let Some(arg) = it.next() {
                match arg.as_str() {
                    "-p" => {
                        if let Some(v) = it.next() {
                            port = v.parse().unwrap_or(22);
                        }
                    }
                    "-i" => identity = it.next().cloned(),
                    _ => {}
                }
            }
            LinkAddress::Ssh {
                host,
                port,
                user,
                identity,
                proxy: None,
            }
        }
    }
}

#[cfg(test)]
mod control_address_tests {
    use super::*;
    use crate::model::{TmuxHostConfig, TmuxSocket};

    fn argv_of(host: &TmuxHostConfig) -> (Vec<String>, Vec<String>) {
        let prefix = crate::tmux::host_prefix(host);
        let argv = crate::tmux::argv(&prefix, &TmuxSocket::Default, "work");
        (prefix.clone(), argv[prefix.len()..].to_vec())
    }

    #[test]
    fn a_local_control_link_is_a_shell() {
        let host = TmuxHostConfig::Local;
        let (_, tmux_argv) = argv_of(&host);
        match control_link_address(&host, &tmux_argv) {
            LinkAddress::Shell { command, args } => {
                assert_eq!(command, "tmux");
                assert!(args.iter().any(|a| a == "attach"), "{args:?}");
            }
            other => panic!("a local tmux link must be a shell link, got {other:?}"),
        }
    }

    #[test]
    fn a_remote_control_link_is_ssh_not_shell() {
        let host = TmuxHostConfig::Ssh {
            destination: "dev@box".into(),
            args: vec![
                "-p".into(),
                "2222".into(),
                "-i".into(),
                "~/.ssh/id_ed25519".into(),
            ],
        };
        let (_, tmux_argv) = argv_of(&host);
        let addr = control_link_address(&host, &tmux_argv);
        assert_eq!(
            addr.kind(),
            "ssh",
            "a remote tmux session is reached over ssh: {addr:?}"
        );
        assert_eq!(
            addr,
            LinkAddress::Ssh {
                host: "box".into(),
                port: 2222,
                user: "dev".into(),
                identity: Some("~/.ssh/id_ed25519".into()),
                proxy: None,
            }
        );
    }

    #[test]
    fn a_destination_without_a_user_still_reads_as_ssh() {
        let host = TmuxHostConfig::Ssh {
            destination: "box".into(),
            args: vec![],
        };
        let (_, tmux_argv) = argv_of(&host);
        match control_link_address(&host, &tmux_argv) {
            LinkAddress::Ssh { host, port, .. } => {
                assert_eq!(host, "box");
                assert_eq!(port, 22);
            }
            other => panic!("expected ssh, got {other:?}"),
        }
    }
}
