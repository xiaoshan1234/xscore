// core/manager.rs (v0.6.0 — Session owns the backend, ADR 0010)
//
// All methods are async; all errors flow through `core::Error`. The
// manager holds `Arc<...>` registries so cloning the manager is cheap.
//
// Creation semantics:
//   - `add_link(spec, addr, id, pane)` yields a `Pipe` link and a usable pane:
//       * `pane: Some(pid)` → binds that existing (hollow) pane;
//       * `pane: None`      → creates a native window + a pane and its
//                             `DirectBridge` (ring buffer).
//     There is no tmux branch: a tmux session is created/adopted at the
//     session level (`create_tmux_session` / `adopt_tmux_session`).
//   - `add_window_in(session, ..)` creates a real tmux window through the
//     session's control client when the session is tmux-backed, else a native
//     window.
//   - `add_pane_in(.., window)` splits inside a real tmux window (through the
//     control client) when the window has a `WindowBackend::Tmux`, else it
//     inserts a hollow pane.
//
// `detach_pane` never touches a shared transport (P0-a) and refuses for panes
// that tmux owns (they are addressed by `link_key`). `del_link` on a `Pipe`
// link orphans its panes and keeps their buffers; a `Control` link cannot be
// deleted at all — `session kill` / `session forget` are the verbs.
//
// v0.6.0 PR3b (ADR 0010 §D2) added tmux-backed sessions: `create_tmux_session`
// / `adopt_tmux_session` spawn ONE `tmux -C attach` client per session, which
// is the session's single `Control` link. Pane bytes arrive as `%output %N`
// and are routed to the pane whose `link_key` is that handle; structure arrives
// as events. PR4 deleted the old per-command tmux path, so the control client is
// only way xscore touches tmux data.
//
// State persistence: `save_state` writes `StateSnapshot` (schema v3);
// see `core::state` for the version gate and the migrations.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use tokio::sync::Mutex;

use super::{Error, LinkAddress, Result};
use super::buffer::{RingBuffer, DIRECT_BUFFER_CAPACITY};
use super::id::{LinkId, PaneId, SessionId, WindowId};
use super::link::Link;
use super::link_registry::LinkRegistry;
use super::pane_registry::PaneRegistry;
use super::session::Session;
use super::session_registry::SessionRegistry;
use super::window::Window;
use super::window_registry::WindowRegistry;

use crate::backends::{ControlBridge, DaemonBridge, DirectBridge, PaneBridge};
use crate::daemon::client::DaemonClient;
use crate::model::{
    ConnectSpec, EndpointRef, PaneInfo, SessionBackend, SessionInfo, TmuxHostConfig, TmuxSocket,
    TransportConfig, WindowBackend, WindowInfo,
};
use crate::tmux::{PaneLine, TmuxControl, TmuxEvent, WindowLine};
use crate::transport::factory as transport_factory;
use crate::transport::NoopTransport;

/// What `link add` produced. A pipe link always comes with a pane (and usually
/// a window), so the CLI can print all three ids.
#[derive(Debug, Clone, Copy)]
pub struct LinkAddResult {
    pub link_id: LinkId,
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
    /// Per-pane bridge map. Stored SEPARATELY from `PaneInfo` so mutating
    /// the info (which is JSON-serializable) doesn't destroy the bridge.
    /// Bridges are in-memory only.
    pane_bridges: Arc<Mutex<HashMap<PaneId, Arc<dyn PaneBridge>>>>,
    /// Per-pane direct-pane buffers. They OUTLIVE the bridge: detaching a
    /// direct pane drops the bridge but keeps the history (ADR 0008).
    /// Only used in in-process mode — with a daemon, the buffer belongs
    /// to the daemon (ADR 0009).
    pane_buffers: Arc<Mutex<HashMap<PaneId, Arc<Mutex<RingBuffer>>>>>,
    /// v0.5.0 (ADR 0009): when set, direct links are daemon-backed —
    /// their transport and pane buffers live in the resident daemon, so
    /// they survive this process. `None` keeps everything in-process
    /// (that is what `data_smoke` exercises).
    daemon: Option<Arc<DaemonClient>>,
    /// v0.6.0 (ADR 0010 §D2): the live control client of every tmux-backed
    /// session this manager has attached. One per session — a tmux session is
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
    fn default() -> Self { Self::new() }
}

impl SessionManager {
    pub fn new() -> Self {
        Self {
            sessions: Arc::new(SessionRegistry::new()),
            links: Arc::new(LinkRegistry::new()),
            windows: Arc::new(WindowRegistry::new()),
            panes: Arc::new(PaneRegistry::new()),
            pane_bridges: Arc::new(Mutex::new(HashMap::new())),
            pane_buffers: Arc::new(Mutex::new(HashMap::new())),
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
        let Some(link) = self.links.get(lid).await else { return false };
        if link.info.is_control() {
            // The attached client is the fast, exact answer; note that a stale
            // handle (closed by `%exit`) must not count as connected.
            if let Some(control) = &link.control {
                if !control.is_closed() {
                    return true;
                }
            }
            let held = { self.tmux_controls.lock().await.get(&link.info.session_id).cloned() };
            if let Some(control) = held {
                return !control.is_closed();
            }
            // No client in this process. Asking the server (one `has-session`)
            // is the honest answer; reporting `false` would call a perfectly
            // healthy session disconnected just because this process did not
            // attach. This is a `show`, and showing is live (arch.md §6).
            let Ok(info) = self.get_session(link.info.session_id).await else { return false };
            return match info.backend {
                SessionBackend::Tmux { host, socket, session } => self
                    .tmux_session_exists(&host, &socket, &session)
                    .await
                    .unwrap_or(false),
                SessionBackend::Native => false,
            };
        }
        match &self.daemon {
            Some(_) => crate::daemon::client::probe_link(lid).await.unwrap_or(false),
            None => link.connected,
        }
    }

    // ===== sessions (v0.6.0, ADR 0010 §1) =====

    /// Which workspace does this command mean?
    ///
    /// `Some(name)` is the user being explicit (`--session work`) and must
    /// never fall back to something else — a typo would silently write into the
    /// wrong workspace. `None` picks the only session if there is exactly one,
    /// else `default`, and on a virgin state creates `default` so that every
    /// pre-v0.6 command keeps working without a session of its own.
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
        if let Some(s) = self.sessions.get_by_name("default").await {
            return Ok(s.info.id);
        }
        self.add_session_native("default").await
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

        if let SessionBackend::Tmux { host, socket, session: name } = &session.info.backend {
            // Destroy means destroy: the control client goes first (so nothing
            // is streaming into rows that are about to disappear), then
            // `kill-session` takes the server session down. Both steps are best
            // effort — a session that is already gone must still lose its rows.
            let control = { self.tmux_controls.lock().await.remove(&id) };
            if let Some(control) = control {
                let _ = control.close().await;
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

        let stragglers: Vec<PaneInfo> = self
            .panes
            .list_all()
            .await
            .into_iter()
            .filter(|p| p.session_id == id)
            .collect();
        for info in &stragglers {
            let bridge = self.pane_bridges.lock().await.remove(&info.id);
            self.pane_buffers.lock().await.remove(&info.id);
            // Releasing a bridge only stops a pump: a direct bridge's `close`
            // is "stop my reader", a control bridge's is a no-op, and the tmux
            // session is already gone either way.
            if let Some(b) = bridge {
                let _ = b.close().await;
            }
            self.panes.remove(info.id).await;
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
        self.save_state().await?;
        Ok(())
    }

    /// Stop managing a session without touching its backend (arch.md §8).
    ///
    /// Only meaningful for tmux: the tmux server keeps running and the session
    /// stays in `tmux ls`; xscore just drops its rows. A native session has no
    /// backend outside this process, so "forget" would silently mean "kill" —
    /// refuse instead.
    pub async fn forget_session(&self, id: SessionId) -> Result<()> {
        let session = self
            .sessions
            .get(id)
            .await
            .ok_or_else(|| Error::SessionNotFound(id.to_string()))?;
        if !session.info.is_tmux() {
            return Err(Error::ForgetNativeSession(id));
        }

        // Detach: the control client is closed, which is what ends the pumps,
        // and tmux keeps running (arch.md §8, "session forget leaves the
        // backend running").
        let control = { self.tmux_controls.lock().await.remove(&id) };
        if let Some(control) = control {
            let _ = control.close().await;
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
        for info in self.panes.list_all().await.iter().filter(|p| p.session_id == id) {
            self.pane_bridges.lock().await.remove(&info.id);
            self.pane_buffers.lock().await.remove(&info.id);
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
    //   * the pane buffers, filled from `%output %N` and routed by `link_key`.
    //
    // Structure arrives as events (`%window-add`, `%layout-change`,
    // `%unlinked-window-close`), never from polling — but a CLI invocation is
    // its own process, so a command that has to *see* the structure re-reads it
    // through `ensure_tmux_attached` (arch.md §7, and ADR 0009's daemon is
    // where that client will eventually live between commands).

    /// Create a real tmux session and manage it.
    ///
    /// The tmux session is named after the xscore session: two names for one
    /// thing would drift, and `session adopt` is the verb for a session that
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
            args.extend(["--".into(), "/bin/sh".into(), "-c".into(), command.to_string()]);
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
    /// The event pump keeps a live manager in step, but this CLI is one
    /// process per command: between commands no client is attached, so a
    /// window created outside xscore is invisible until something attaches.
    /// Listings call this for exactly that reason, and attaching is also what
    /// gives a migrated session its control link (see `ensure_control_link`).
    pub async fn refresh_tmux_session(&self, id: SessionId) -> Result<()> {
        self.ensure_tmux_attached(id).await.map(|_| ())
    }

    /// The session's control client, attached on demand.
    ///
    /// Attaching is also the reconnect path: after `%exit` (the client died,
    /// the ssh channel dropped) the handle is closed, so it is dropped and
    /// replaced. Output produced while nothing was attached is gone — tmux
    /// replays nothing (ADR 0010 §D2) — but the session itself keeps running.
    pub async fn ensure_tmux_attached(&self, id: SessionId) -> Result<Arc<TmuxControl>> {
        // Held across the check-and-spawn: see `tmux_attach`.
        let _guard = self.tmux_attach.lock().await;
        let existing = { self.tmux_controls.lock().await.get(&id).cloned() };
        if let Some(control) = existing {
            if !control.is_closed() {
                return Ok(control);
            }
            self.tmux_controls.lock().await.remove(&id);
        }
        self.attach_tmux(id).await
    }

    /// Spawn the control client, enumerate the session, start the pumps.
    ///
    /// The caller serialises attaches (`tmux_attach`); this is not reentrant.
    /// A session that has no control link yet (one whose rows were converted by
    /// `state migrate`) gets it here: attaching IS what creates it.
    async fn attach_tmux(&self, id: SessionId) -> Result<Arc<TmuxControl>> {
        self.ensure_control_link(id).await?;
        let info = self.get_session(id).await?;
        let SessionBackend::Tmux { host, socket, session } = info.backend else {
            return Err(Error::NotTmuxSession(id));
        };
        let prefix = crate::tmux::host_prefix(&host);
        let control = Arc::new(TmuxControl::spawn(&prefix, &socket, &session)?);

        // Subscribe BEFORE enumerating: `%output` is not replayed, so a pane
        // added between the enumeration and the subscription would stream into
        // nothing until it next prints.
        let events = control.subscribe();

        if let Err(e) = self.reconcile_all(id, &control).await {
            // Do not leave a client attached to a session we could not adopt.
            let _ = control.close().await;
            // The client dying during the handshake means one of two things —
            // the session is gone, or the host never answered — and `attach`
            // cannot tell them apart (only a one-shot's exit 255 can, arch.md
            // §6). Say what happened instead of surfacing the raw "closed".
            return Err(Error::Sink(format!(
                "could not attach to tmux session `{session}` on {}: {e}",
                crate::tmux::host_label(&host)
            )));
        }

        self.tmux_controls.lock().await.insert(id, Arc::clone(&control));
        self.record_control(id, &control).await;
        self.start_event_pump(id, Arc::clone(&control), events);
        Ok(control)
    }

    /// Note the live client on the session's control link (ADR 0010 §4: the
    /// link IS the connection, so the handle belongs on the link row as well as
    /// in the session-keyed map). Best effort: a session that has no control
    /// link yet (attachment happens before the row exists on the very first
    /// attach) simply has nothing to record.
    async fn record_control(&self, id: SessionId, control: &Arc<TmuxControl>) {
        let Some(link_id) = self.control_link_id(id).await else { return };
        let Some(existing) = self.links.get(link_id).await else { return };
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
            .add_session(SessionInfo::new_tmux(name, host.clone(), socket.clone(), name))
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
            let _ = self.forget_session(sid).await;
            if created {
                let _ = self
                    .run_tmux(
                        &host,
                        &socket,
                        &["kill-session".to_string(), "-t".to_string(), name.to_string()],
                    )
                    .await;
            }
            return Err(e);
        }
        Ok(sid)
    }

    /// Create the session's single link and attach its control client.
    async fn open_control_link(&self, id: SessionId) -> Result<LinkId> {
        let link_id = self.ensure_control_link(id).await?;
        self.attach_tmux(id).await?;
        Ok(link_id)
    }

    /// Make sure the session has its control link row, and return its id.
    ///
    /// Idempotent: this is the one place that materialises the link, so a
    /// session created by `session new --tmux`, one adopted, and one whose rows
    /// came from `state migrate` all end up with exactly one.
    async fn ensure_control_link(&self, id: SessionId) -> Result<LinkId> {
        if let Some(link_id) = self.control_link_id(id).await {
            return Ok(link_id);
        }
        let info = self.get_session(id).await?;
        let SessionBackend::Tmux { host, socket, session } = info.backend else {
            return Err(Error::NotTmuxSession(id));
        };
        let link_id = LinkId::new();
        // The link is the connection to the *server*, so the process is the
        // connection: `address` records the argv that spawned it (readable in
        // `link show`), while the live client is the `TmuxControl` handle in
        // `control`/`tmux_controls`. Neither a transport nor a sink is ever
        // used to move these bytes — the endpoints's kind says so.
        let argv = crate::tmux::argv(&crate::tmux::host_prefix(&host), &socket, &session);
        let mut words = argv.into_iter();
        let command = words.next().unwrap_or_else(|| "tmux".to_string());
        let args: Vec<String> = words.collect();

        let link = Link {
            id: link_id,
            info: crate::model::LinkInfo {
                id: link_id,
                session_id: id,
                name: format!("{session}-ctl"),
                created_at: chrono::Utc::now(),
                address: LinkAddress::Shell { command, args },
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

    /// Re-read every window and pane of the session.
    ///
    /// Idempotent: rows are matched by handle (`@N`/`%N`, never the index), so
    /// running it twice changes nothing. Rows tmux no longer has are dropped.
    async fn reconcile_all(&self, id: SessionId, control: &Arc<TmuxControl>) -> Result<bool> {
        let info = self.get_session(id).await?;
        let SessionBackend::Tmux { session, .. } = info.backend else {
            return Err(Error::NotTmuxSession(id));
        };
        let _guard = self.tmux_reconcile.lock().await;

        let lines = control
            .send(&crate::tmux::list_windows_command(&session))
            .await?;
        let windows: Vec<WindowLine> = lines
            .iter()
            .filter_map(|line| crate::tmux::parse_window_line(line))
            .collect();

        let mut changed = false;
        let mut seen: HashSet<String> = HashSet::new();
        for window in &windows {
            seen.insert(window.handle.clone());
            changed |= self.sync_window(id, control, window).await?;
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
            self.save_state().await?;
        }
        Ok(changed)
    }

    /// Re-read one window (an event's answer to "what changed?").
    async fn reconcile_window(
        &self,
        id: SessionId,
        control: &Arc<TmuxControl>,
        handle: &str,
    ) -> Result<bool> {
        let info = self.get_session(id).await?;
        let SessionBackend::Tmux { session, .. } = info.backend else {
            return Err(Error::NotTmuxSession(id));
        };
        let _guard = self.tmux_reconcile.lock().await;

        let lines = control
            .send(&crate::tmux::list_windows_command(&session))
            .await?;
        let window = lines
            .iter()
            .filter_map(|line| crate::tmux::parse_window_line(line))
            .find(|w| w.handle == handle);
        let Some(window) = window else {
            // Gone between the notification and the query: treat it as the
            // close it already is.
            let changed = self.forget_window_by_handle(id, handle).await;
            if changed {
                self.save_state().await?;
            }
            return Ok(changed);
        };

        let changed = self.sync_window(id, control, &window).await?;
        if changed {
            self.save_state().await?;
        }
        Ok(changed)
    }

    /// Upsert one window and its panes; start an output pump for each pane that
    /// did not exist before.
    ///
    /// The caller holds `tmux_reconcile`.
    async fn sync_window(
        &self,
        id: SessionId,
        control: &Arc<TmuxControl>,
        window: &WindowLine,
    ) -> Result<bool> {
        let Some(link_id) = self.control_link_id(id).await else {
            return Err(Error::Sink(format!(
                "session {id} is tmux-backed but has no control link"
            )));
        };
        let mut changed = false;

        let wid = match self.window_for_handle_in(id, &window.handle).await {
            Some(wid) => {
                let Some(arc) = self.windows.get(wid).await else { return Ok(false) };
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
                    backend: WindowBackend::Tmux { handle: window.handle.clone() },
                };
                self.windows.insert(Window::new(info)).await;
                changed = true;
                wid
            }
        };

        let lines = control
            .send(&crate::tmux::list_panes_command(&window.handle))
            .await?;
        let panes: Vec<PaneLine> = lines
            .iter()
            .filter_map(|line| crate::tmux::parse_pane_line(line))
            .collect();

        let mut seen: HashSet<String> = HashSet::new();
        for pane in &panes {
            seen.insert(pane.handle.clone());
            match self.pane_by_link_key(id, &pane.handle).await {
                Some(existing) => {
                    let mut updated = existing.clone();
                    let before = updated.clone();
                    updated.cols = pane.cols;
                    updated.rows = pane.rows;
                    updated.x = pane.x;
                    updated.y = pane.y;
                    updated.window_id = Some(wid);
                    if updated.link_id.is_none() {
                        updated.link_id = Some(link_id);
                    }
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
                    let info = PaneInfo {
                        id: pid,
                        session_id: id,
                        name: format!("p-{pid}"),
                        created_at: chrono::Utc::now(),
                        cols: pane.cols,
                        rows: pane.rows,
                        x: pane.x,
                        y: pane.y,
                        link_id: Some(link_id),
                        window_id: Some(wid),
                        link_key: Some(pane.handle.clone()),
                    };
                    self.panes.insert(info).await;
                    if let Some(arc) = self.windows.get(wid).await {
                        arc.panes.lock().await.push(pid);
                    }
                    // A pane with no subscriber loses its bytes: tmux replays
                    // nothing, so the pump starts with the row.
                    self.start_output_pump(Arc::clone(control), pid, pane.handle.clone())
                        .await;
                    changed = true;
                }
            }
        }

        // Panes this window no longer has.
        let stale: Vec<PaneId> = self
            .panes
            .list_all()
            .await
            .iter()
            .filter(|p| p.session_id == id && p.window_id == Some(wid))
            .filter(|p| match &p.link_key {
                Some(key) => !seen.contains(key),
                None => false,
            })
            .map(|p| p.id)
            .collect();
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

    async fn pane_by_link_key(&self, id: SessionId, key: &str) -> Option<PaneInfo> {
        self.panes
            .list_all()
            .await
            .into_iter()
            .find(|p| p.session_id == id && p.link_key.as_deref() == Some(key))
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
        let bridge = self.pane_bridges.lock().await.remove(&pid);
        if let Some(bridge) = bridge {
            let _ = bridge.close().await;
        }
        self.pane_buffers.lock().await.remove(&pid);
        self.panes.remove(pid).await;
    }

    // ----- pumps -----

    /// One task per session: structure changes become registry rows.
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
                match event {
                    // A rename and a layout change are the same question as an
                    // add: "what does this window look like now?"
                    TmuxEvent::WindowAdd { handle }
                    | TmuxEvent::WindowRenamed { handle }
                    | TmuxEvent::LayoutChange { handle, .. } => {
                        if let Err(e) = manager.reconcile_window(id, &control, &handle).await {
                            tracing::warn!("tmux: could not reconcile window {handle}: {e}");
                        }
                    }
                    TmuxEvent::WindowClose { handle } => {
                        if manager.forget_window_by_handle(id, &handle).await {
                            if let Err(e) = manager.save_state().await {
                                tracing::warn!("tmux: could not save state after {handle} closed: {e}");
                            }
                        }
                    }
                    // Nothing in xscore stores "the current window" yet, so
                    // these are informational (arch.md §7 lists them as state
                    // to update when a renderer exists).
                    TmuxEvent::SessionWindowChanged { session, window } => {
                        tracing::debug!("tmux {id}: current window of {session} is {window}");
                    }
                    TmuxEvent::SessionChanged { session } => {
                        tracing::debug!("tmux {id}: session is {session}");
                    }
                    TmuxEvent::SessionsChanged => {
                        tracing::debug!("tmux {id}: the session set changed");
                    }
                    TmuxEvent::Exit { reason } => {
                        // The client is the session's single connection: with
                        // it gone the link is disconnected (which
                        // `link_is_connected` reports from the closed handle)
                        // and every pane pump ends, because `%exit` shuts the
                        // subscriber channels down.
                        tracing::info!("tmux: control client for session {id} exited: {reason}");
                        break;
                    }
                }
            }
        });
    }

    /// One task per pane: `%output %N` appended to that pane's buffer.
    async fn start_output_pump(&self, control: Arc<TmuxControl>, pid: PaneId, key: String) {
        let buffer = self.buffer_for(pid).await;
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

    // ----- one-shot tmux commands (arch.md §6, "two exec shapes") -----

    async fn tmux_session_exists(
        &self,
        host: &TmuxHostConfig,
        socket: &TmuxSocket,
        name: &str,
    ) -> Result<bool> {
        let out = self
            .run_tmux(host, socket, &["has-session".to_string(), "-t".to_string(), name.to_string()])
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
                reason: if stderr.is_empty() { "exit 255".into() } else { stderr },
            };
        }
        Error::Sink(format!(
            "tmux {what} -t {session} failed ({}): {stderr}",
            out.status.code().unwrap_or(-1)
        ))
    }

    // ===== add_link =====

    /// Create a link and give it a usable pane, in the default session.
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
            if let Some(existing) = info.link_id {
                return Err(Error::PaneAlreadyAttached(pid, existing));
            }
            if let Some(key) = info.link_key.clone() {
                // A pane that carries a `%N` belongs to tmux and is addressed on
                // its session's control link; there is nothing here to re-bind.
                return Err(Error::Invalid(format!(
                    "pane {pid} is addressed by {key} on its session's control link; \
                     it cannot be attached to a pipe link"
                )));
            }
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
        if let Some(client) = &daemon {
            client.ensure().await?;
            client.create_link(link_id, spec.transport.clone()).await?;
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
                    Some(client) => {
                        client.create_pane(pid).await?;
                        Arc::new(DaemonBridge::new(Arc::clone(client), pid))
                    }
                    None => {
                        let buf = self.buffer_for(pid).await;
                        Arc::new(DirectBridge::new_with_buffer(
                            Arc::clone(&transport_arc), buf,
                        ))
                    }
                };
                let mut info = self.panes.get(pid).await.ok_or(Error::PaneNotFound(pid))?;
                info.link_id = Some(link_id);
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
                    Some(client) => {
                        client.create_pane(pid).await?;
                        Arc::new(DaemonBridge::new(Arc::clone(client), pid))
                    }
                    None => {
                        let buf = self.buffer_for(pid).await;
                        Arc::new(DirectBridge::new_with_buffer(
                            Arc::clone(&transport_arc), buf,
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
                        link_id: Some(link_id),
                        window_id: Some(wid),
                        link_key: None,
                    },
                    Some(bridge),
                    Some(w),
                )
            }
        };

        let link_info = crate::model::LinkInfo {
            id: link_id,
            session_id: session,
            name: if spec.name.is_empty() { link_id.to_string() } else { spec.name.clone() },
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
            self.pane_bridges.lock().await.insert(pane_id, b);
        }

        // ADR 0009: the binding only becomes LIVE at Attach — the daemon
        // starts the pane's pump here, and keeps it running after this
        // process exits. A failure here must not leave an orphaned
        // transport behind, so tear the daemon link back down.
        if let Some(client) = &daemon {
            if let Err(e) = client.attach(pane_id, link_id).await {
                let _ = client.delete_link(link_id).await;
                return Err(e);
            }
        }

        if let Err(e) = self.save_state().await {
            // A failed save means the CLI exits with an error, so nothing
            // will ever reference this link. Do not leave its transport
            // (and a daemon that can therefore never idle-exit) behind.
            if let Some(client) = &daemon {
                let _ = client.delete_link(link_id).await;
            }
            return Err(e);
        }
        Ok(LinkAddResult { link_id, window_id, pane_id })
    }

    /// Full teardown of a link.
    ///
    /// * pipe link: orphan every attached pane (buffer kept), close the
    ///   transport ONCE, remove the link.
    /// * control link: refused (ADR 0010 §D5) — see `del_session` /
    ///   `forget_session` for the two verbs that do tear a tmux session down.
    pub async fn del_link(&self, id: LinkId) -> Result<()> {
        let arc_link = self.links.get(id).await.ok_or(Error::LinkNotFound(id))?;

        // ADR 0010 §D5: every pane in a tmux session routes through its control
        // link, so deleting it would leave them with buffers and no way back.
        if arc_link.info.is_control() {
            return Err(Error::ControlLinkNotDeletable(id));
        }

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
                // DirectBridge::close() stops the pump only (in daemon
                // mode: Detach), so the transport below is closed
                // exactly once, on purpose.
                let _ = b.close().await;
            }
            if let Some(mut info) = self.panes.get(pid).await {
                info.link_id = None;
                self.panes.insert(info).await;
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
        self.save_state().await?;
        Ok(())
    }

    // ===== add_window / del_window =====

    /// Create a window in the default session. See `add_window_in`.
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
                let control = self.ensure_tmux_attached(session).await?;
                let lines = control
                    .send(&crate::tmux::new_window_command(name.as_deref()))
                    .await?;
                let handle = tmux_handle_from(&lines, '@').ok_or_else(|| {
                    Error::Sink(format!(
                        "tmux new-window did not report a window handle: {lines:?}"
                    ))
                })?;
                // Read the new window back: the row must be tmux's view of it,
                // not our guess at its geometry or name.
                self.reconcile_window(session, &control, &handle).await?;
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

    // ===== add_pane / del_pane =====

    /// Create a pane in the default session. See `add_pane_in`.
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
    ///   reconciliation creates, addressed by its `%N` `link_key` on the
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

        // A tmux window's panes are tmux's: split it and adopt what tmux
        // creates, rather than inventing a hollow row that no byte could reach.
        if let Some(wid) = window {
            let arc_window = self.windows.get(wid).await.ok_or(Error::WindowNotFound(wid))?;
            if arc_window.info.session_id != session {
                return Err(Error::NotInSession {
                    session,
                    what: "window",
                    id: wid.to_string(),
                });
            }
            if let Some(handle) = arc_window.info.backend.tmux_handle().map(str::to_string) {
                let control = self.ensure_tmux_attached(session).await?;
                let lines = control
                    .send(&crate::tmux::split_window_command(&handle))
                    .await?;
                let key = tmux_handle_from(&lines, '%').ok_or_else(|| {
                    Error::Sink(format!(
                        "tmux split-window did not report a pane handle: {lines:?}"
                    ))
                })?;
                self.reconcile_window(session, &control, &handle).await?;
                let existing = self.pane_by_link_key(session, &key).await.ok_or_else(|| {
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
            link_id: None,
            window_id: window,
            link_key: None,
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
        // Drop the bridge if attached. Releasing a bridge stops a pump; a
        // control pane is killed on the server further down.
        let bridge = self.pane_bridges.lock().await.remove(&id);
        if let Some(b) = bridge {
            let _ = b.close().await;
        }
        self.pane_buffers.lock().await.remove(&id);
        // ADR 0009: drop the daemon's slot (and its buffer) too — `pane
        // del` is a full teardown, unlike `pane detach`.
        if let Some(client) = &self.daemon {
            let _ = client.delete_pane(id).await;
        }
        // A pane tmux owns is killed on the server: dropping only the row would
        // leave a `%N` that the next reconciliation adopts right back.
        if let Some(key) = info.link_key.clone() {
            if let Ok(control) = self.ensure_tmux_attached(info.session_id).await {
                if let Err(e) = control
                    .send(&crate::tmux::kill_pane_command(&key))
                    .await
                {
                    // Already gone is the state we wanted.
                    tracing::debug!("pane del: kill-pane {key} failed: {e}");
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

    // ===== attach_pane / detach_pane (exclusivity, per link kind) =====

    /// Bind a pane to a `Pipe` link.
    ///
    /// Exclusivity is per link kind (ADR 0010 §D3): a pipe link carries one byte
    /// stream, so at most one pane may read it — attaching auto-detaches the
    /// previous reader (its buffer survives). A `Control` link is not something
    /// a pane can be attached to: its panes arrive from tmux with a `%N`.
    pub async fn attach_pane(&self, pid: PaneId, lid: LinkId) -> Result<()> {
        let arc_link = self.links.get(lid).await.ok_or(Error::LinkNotFound(lid))?;
        let info = self.panes.get(pid).await.ok_or(Error::PaneNotFound(pid))?;

        // v0.6.0 (ADR 0010 §8): a pane may only be bound to a link of its own
        // session. Without this, `pane attach` could move a pane into another
        // workspace's byte stream and the two would disagree about who owns it.
        if info.session_id != arc_link.info.session_id {
            return Err(Error::NotInSession {
                session: arc_link.info.session_id,
                what: "pane",
                id: pid.to_string(),
            });
        }
        if arc_link.info.is_control() {
            return Err(Error::Invalid(format!(
                "link {lid} is the session's tmux control link; a pane reaches it by \
                 being created in tmux (its `%N` is its address), not by attaching"
            )));
        }

        if info.link_id == Some(lid) {
            return Ok(());
        }
        if let Some(existing) = info.link_id {
            return Err(Error::PaneAlreadyAttached(pid, existing));
        }
        if let Some(key) = info.link_key.as_deref() {
            // A pane on a control link is addressed by `%N` on that link and
            // nothing else (ADR 0010 §D6).
            return Err(Error::Invalid(format!(
                "pane {pid} is addressed by {key} on its session's control link; \
                 it cannot be attached to a pipe link"
            )));
        }

        // One reader per pipe: detach whatever pane was reading this link.
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
                "auto-detached pane {old_pid} from link {lid} (new attach: {pid})"
            );
        }

        // Build the bridge.
        let bridge: Arc<dyn PaneBridge> = match &self.daemon {
            // ADR 0009: the daemon owns the pump and the buffer. It
            // enforces the same one-reader rule locally (it must — two pumps
            // on one transport would duplicate every byte into two
            // buffers), so the metadata loop above and the daemon
            // agree on which pane is active.
            Some(client) => {
                if client.attach(pid, lid).await.is_err() {
                    // The daemon may have restarted since this link
                    // was created. Heal it from the persisted
                    // transport spec, then bind again — this is what
                    // makes `pane attach` a reconnect, not just a
                    // metadata edit.
                    client.ensure().await?;
                    client
                        .create_link(lid, pipe_transport(&arc_link.info)?)
                        .await?;
                    client.attach(pid, lid).await?;
                }
                Arc::new(DaemonBridge::new(Arc::clone(client), pid))
            }
            None => {
                let buf = self.buffer_for(pid).await;
                Arc::new(DirectBridge::new_with_buffer(
                    Arc::clone(&arc_link.transport),
                    buf,
                ))
            }
        };

        self.pane_bridges.lock().await.insert(pid, bridge);

        if let Some(mut i) = self.panes.get(pid).await {
            i.link_id = Some(lid);
            self.panes.insert(i).await;
        }

        self.save_state().await?;
        Ok(())
    }

    /// Detach a pane from its link. A pipe pane keeps its buffer; a tmux pane
    /// cannot be detached at all (its `%N` is its identity on the server).
    pub async fn detach_pane(&self, pid: PaneId) -> Result<()> {
        let info = self.panes.get(pid).await.ok_or(Error::PaneNotFound(pid))?;
        // A tmux pane cannot be detached, only killed (arch.md §8).
        if info.link_key.is_some() {
            return Err(Error::TmuxPaneNotDetachable(pid));
        }
        let _lid = match info.link_id {
            Some(l) => l,
            None => return Err(Error::NotAttached(pid)),
        };
        let bridge = self.pane_bridges.lock().await.remove(&pid);
        if let Some(b) = bridge {
            // P0-a: releases this pane's pump; the link's transport lives.
            let _ = b.close().await;
        }
        // ADR 0009: also tell the daemon directly — a hydrated pane has no
        // cached bridge, but its pump in the daemon may still be running.
        if let Some(client) = &self.daemon {
            let _ = client.detach(pid).await;
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
        let target = self.windows.get(wid).await.ok_or(Error::WindowNotFound(wid))?;
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
        if let Some(bridge) = self.pane_bridges.lock().await.get(&pid).cloned() {
            bridge.resize(cols, rows).await?;
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
        if let Some(b) = self.pane_bridges.lock().await.get(&pid).cloned() {
            return Ok(b);
        }
        let info = self.panes.get(pid).await.ok_or(Error::PaneNotFound(pid))?;

        // A pane that carries a `link_key` belongs to a control link: tmux is
        // already resident and holds its scrollback, so it is never
        // daemon-backed and comes first.
        if let Some(b) = self.control_bridge(&info).await? {
            self.pane_bridges.lock().await.insert(pid, Arc::clone(&b));
            return Ok(b);
        }

        if let Some(client) = &self.daemon {
            return match info.link_id {
                Some(lid) => {
                    if crate::daemon::client::probe_link(lid).await.unwrap_or(false) {
                        Ok(Arc::new(DaemonBridge::new(Arc::clone(client), pid)))
                    } else {
                        Err(Error::LinkNotConnected(lid))
                    }
                }
                None => {
                    // Detached direct pane: the daemon keeps its buffer so
                    // the scrollback stays READABLE (a pane's core is its
                    // buffer), while writes are refused there for want of
                    // an attached link.
                    if crate::daemon::client::probe_pane(pid).await.unwrap_or(false) {
                        Ok(Arc::new(DaemonBridge::new(Arc::clone(client), pid)))
                    } else {
                        Err(Error::NotAttached(pid))
                    }
                }
            };
        }

        match info.link_id {
            Some(lid) => {
                let connected = self
                    .links
                    .get(lid)
                    .await
                    .map(|l| l.connected)
                    .unwrap_or(false);
                if connected {
                    // Connected link but no bridge: a bug, not a user
                    // state. Surface it as NotAttached rather than
                    // silently returning zero bytes.
                    Err(Error::NotAttached(pid))
                } else {
                    Err(Error::LinkNotConnected(lid))
                }
            }
            None => Err(Error::NotAttached(pid)),
        }
    }

    /// The bridge for a pane that lives on its session's control link, if it
    /// does.
    ///
    /// Attaching here — rather than only at creation — is what makes
    /// `data read/write` work from a process that did nothing but load
    /// `state.json` (arch.md §7: the structure is live, not a mirror).
    async fn control_bridge(&self, info: &PaneInfo) -> Result<Option<Arc<dyn PaneBridge>>> {
        let (Some(link_id), Some(key)) = (info.link_id, info.link_key.clone()) else {
            return Ok(None);
        };
        let Some(link) = self.links.get(link_id).await else { return Ok(None) };
        if !link.info.is_control() {
            return Ok(None);
        }
        let control = self.ensure_tmux_attached(info.session_id).await?;
        let buffer = self.buffer_for(info.id).await;
        Ok(Some(Arc::new(ControlBridge::new(control, key, buffer))))
    }

    // ===== internal helpers =====

    /// The per-pane buffer, created on first use and kept across
    /// detach/re-attach.
    async fn buffer_for(&self, pid: PaneId) -> Arc<Mutex<RingBuffer>> {
        let mut map = self.pane_buffers.lock().await;
        map.entry(pid)
            .or_insert_with(|| Arc::new(Mutex::new(RingBuffer::new(DIRECT_BUFFER_CAPACITY))))
            .clone()
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

        let snapshot = super::state::StateSnapshot {
            schema_version: super::state::SCHEMA_VERSION,
            sessions,
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
        // v0.6.0 (ADR 0010 D8): sessions BEFORE their children — a link's
        // `session_id` must resolve, and there is deliberately no fallback that
        // invents a session on load.
        for info in snapshot.sessions {
            mgr.sessions.insert(Session::new(info)).await;
        }
        for info in snapshot.links {
            if mgr.sessions.get(info.session_id).await.is_none() {
                tracing::warn!(
                    "hydration: skipping link {} — no session {} in state",
                    info.id, info.session_id
                );
                continue;
            }
            mgr.hydrate_link(info).await;
        }
        for ws in snapshot.windows {
            if mgr.sessions.get(ws.info.session_id).await.is_none() {
                tracing::warn!(
                    "hydration: skipping window {} — no session {} in state",
                    ws.info.id, ws.info.session_id
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
                    pinfo.id, pinfo.session_id
                );
                continue;
            }
            mgr.panes.insert(pinfo).await;
        }
        // Nothing else is rebuilt here on purpose. A pipe link's transport died
        // with the process that owned it (see `Link::connected`: false is the
        // honest answer, so data ops say `LinkNotConnected`), and a control
        // link has no client until this process attaches — `bridge_for` and
        // `ensure_tmux_attached` build both on first use, which is what makes a
        // fresh CLI process work against a session it never created.
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
