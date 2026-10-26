// service/session_service/service.rs (v0.6.0 — Session owns the backend, ADR 0010)
//
// All methods are async; all errors flow through the crate-level `Error`. The
// manager holds `Arc<...>` registries so cloning the manager is cheap.
//
// Creation semantics:
//   - `add_link(spec, addr, id, pane)` yields a `Pipe` Link, an independent
//     Buffer, and a usable set of UI rows:
//       * `pane: Some(pid)` → binds that existing (hollow) pane;
//       * `pane: None`      → creates a native window + a pane and its
//                             and its direct runtime runtime.
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
// transport. `del_link` on a `Pipe` Link stops its SessionService route and
// keeps Buffers and Pane rows; a `Control` link cannot be
// deleted at all — `session kill` / `session detach` are the verbs.
//
// v0.6.0 PR3b (ADR 0010 §D2) added tmux-backed sessions: `create_tmux_session`
// / `adopt_tmux_session` give a session ONE `tmux -CC attach` client, which is
// the session's single `Control` link. The Tmux SessionService maps `%output
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
// see `service/persistence/mod.rs` for the version gate and the migrations.

use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;

use crate::infra::link::{LinkAddress, TransportConfig};
use crate::infra::tmux::{TmuxHostConfig, TmuxSocket};
use crate::service::backend::BackendRuntime;
use crate::service::buffer_service::{BufferId, BufferInfo};
use crate::service::buffer_service::{BufferService, RingBuffer};
use crate::service::link_service::LinkService;
use crate::service::link_service::{EndpointRef, LinkId};
use crate::service::session_service::{
    Session, SessionBackend, SessionId, SessionInfo, SessionRegistry,
};
use crate::service::ui_projection::UiProjection;
use crate::ui::{PaneId, WindowId};
use crate::{Error, Result};

use crate::infra::tmux::{PaneLine, TmuxControl, WindowLine};
use crate::service::daemon::client::DaemonClient;

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
pub struct SessionService {
    /// v0.6.0 (ADR 0010 §1): the workspaces. Children point at a session by
    /// id; this registry is the only place the names live.
    pub sessions: Arc<SessionRegistry>,
    pub link_service: Arc<LinkService>,
    pub ui: Arc<UiProjection>,
    pub buffer_service: Arc<BufferService>,
    pub(super) backends: Arc<Mutex<HashMap<SessionId, BackendRuntime>>>,
    /// Resident daemon client. Buffer runtimes and byte history are owned by
    /// BufferService in-process; in daemon mode the daemon owns both.
    pub(super) daemon: Option<Arc<DaemonClient>>,
    /// v0.6.0 (ADR 0010 §D2): the live control client of every tmux-backed
    /// session this process attached, **in-process mode only**. With a daemon
    /// the client belongs to the daemon (`tmux: HashMap<SessionId,
    /// ControlSlot>` there), because a client that dies with the CLI cannot
    /// stream anything into the next one. One per session — a tmux session is
    /// driven by exactly one client, and a second subscriber for the same
    /// `%output %N` would append every byte twice.
    pub(super) tmux_controls: Arc<Mutex<HashMap<SessionId, Arc<TmuxControl>>>>,
    /// Serialises structure reconciliation. The event pump and a command can
    /// both reconcile at once (a `%window-add` arriving during `window ls`),
    /// and two reconciles racing would insert two rows for one `%N`.
    pub(super) tmux_reconcile: Arc<Mutex<()>>,
    /// Serialises *attaching*. Two clients on one tmux session would mean two
    /// subscribers for the same `%output %N`, i.e. every byte appended twice —
    /// the one-mechanism rule, one level down.
    pub(super) tmux_attach: Arc<Mutex<()>>,
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

impl Default for SessionService {
    fn default() -> Self {
        Self::new()
    }
}

impl SessionService {
    pub fn new() -> Self {
        let buffer_service = Arc::new(BufferService::new());
        Self {
            sessions: Arc::new(SessionRegistry::new()),
            link_service: Arc::new(LinkService::new()),
            ui: Arc::new(UiProjection::new()),
            buffer_service,
            backends: Arc::new(Mutex::new(HashMap::new())),
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
    /// install a `daemon BufferRuntime`, so the link keeps working after this
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
        let Some(link) = self.link_service.get(lid).await else {
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
                if let Ok(alive) = crate::service::daemon::client::probe_tmux(sid).await {
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
            Some(_) => crate::service::daemon::client::probe_link(lid)
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
    pub(super) async fn require_session(&self, id: SessionId) -> Result<()> {
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
            .link_service
            .list()
            .await
            .iter()
            .filter(|l| l.info.session_id == id)
            .map(|l| l.id)
            .collect();
        for lid in link_ids {
            let is_control = match self.link_service.get(lid).await {
                Some(link) => link.info.is_control(),
                None => continue,
            };
            if is_control {
                // Nothing to tear down: the client is closed and its panes are
                // swept below. `del_link` would refuse it anyway.
                self.link_service.remove(lid).await;
                continue;
            }
            self.del_link(lid).await?;
        }

        let straggling_buffers: Vec<BufferInfo> = self
            .buffer_service
            .list()
            .await
            .into_iter()
            .filter(|buffer| buffer.session_id == id)
            .collect();
        for info in &straggling_buffers {
            let runtime = self.buffer_service.take_runtime(info.id).await;
            self.buffer_service.remove_memory(info.id).await;
            // Releasing a runtime only stops a pump: a direct runtime's `close`
            // is "stop my reader", a control runtime's is a no-op, and the tmux
            // session is already gone either way.
            if let Some(b) = runtime {
                let _ = b.close().await;
            }
            self.buffer_service.remove(info.id).await;
            if let Some(client) = &self.daemon {
                let _ = client.delete_buffer(info.id).await;
            }
        }
        let pane_ids: Vec<PaneId> = self
            .ui
            .panes()
            .await
            .into_iter()
            .filter(|pane| pane.session_id == id)
            .map(|pane| pane.id)
            .collect();
        for pid in pane_ids {
            self.ui.remove_pane(pid).await;
        }

        let doomed_windows: Vec<WindowId> = self
            .ui
            .windows()
            .await
            .iter()
            .filter(|w| w.info.session_id == id)
            .map(|w| w.info.id)
            .collect();
        for wid in doomed_windows {
            self.ui.remove_window(wid).await;
        }

        self.sessions.remove(id).await;
        self.backends.lock().await.remove(&id);
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
        // belongs to tmux. Dropping the rows (and the runtimes) is the whole
        // operation.
        let link_ids: Vec<LinkId> = self
            .link_service
            .list()
            .await
            .iter()
            .filter(|l| l.info.session_id == id)
            .map(|l| l.id)
            .collect();
        for lid in link_ids {
            self.link_service.remove(lid).await;
        }
        let buffers: Vec<BufferId> = self
            .buffer_service
            .list()
            .await
            .into_iter()
            .filter(|buffer| buffer.session_id == id)
            .map(|buffer| buffer.id)
            .collect();
        for buffer_id in buffers {
            let _ = self.buffer_service.take_runtime(buffer_id).await;
            self.buffer_service.remove_memory(buffer_id).await;
            self.buffer_service.remove(buffer_id).await;
        }
        let pane_ids: Vec<PaneId> = self
            .ui
            .panes()
            .await
            .iter()
            .filter(|p| p.session_id == id)
            .map(|p| p.id)
            .collect();
        for pid in pane_ids {
            self.ui.remove_pane(pid).await;
        }
        let window_ids: Vec<WindowId> = self
            .ui
            .windows()
            .await
            .iter()
            .filter(|w| w.info.session_id == id)
            .map(|w| w.info.id)
            .collect();
        for wid in window_ids {
            self.ui.remove_window(wid).await;
        }
        self.sessions.remove(id).await;
        self.backends.lock().await.remove(&id);
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
    //     SessionService's `%N -> BufferId` map.
    //
    // Where the client lives is the one thing PR5 changed: with a daemon it is
    // the daemon's (so the buffers outlive this process and `data read` is a
    // stream), without one it is this process's. Structure is therefore
    // **enumerated on touch**: a one-shot CLI has no event subscription between
    // commands, so a listing asks for the structure through whichever client
    // holds it. Events still drive the in-process pumps (and liveness, in the
    // daemon: `%exit` flips the slot's `alive` flag).

    // ===== internal helpers =====

    /// The buffer, created on first use and kept independently of its UI panes.
    /// detach/re-attach.
    pub(super) async fn buffer_for(&self, buffer_id: BufferId) -> Arc<Mutex<RingBuffer>> {
        self.buffer_service.ring(buffer_id).await
    }

    pub(super) async fn rebuild_backend(&self, session_id: SessionId) -> Result<()> {
        let info = self.get_session(session_id).await?;
        let links: Vec<LinkId> = self
            .link_service
            .list()
            .await
            .into_iter()
            .filter(|link| link.info.session_id == session_id)
            .map(|link| link.id)
            .collect();
        let buffers = self.buffer_service.list_for_session(session_id).await;
        if links.is_empty() {
            self.backends.lock().await.remove(&session_id);
            return Ok(());
        }
        let manager = BackendRuntime::from_parts(&info, &links, &buffers)?;
        self.backends.lock().await.insert(session_id, manager);
        Ok(())
    }
}

pub(super) struct TmuxTarget {
    pub(super) host: TmuxHostConfig,
    pub(super) socket: TmuxSocket,
    pub(super) name: String,
    pub(super) cols: u16,
    pub(super) rows: u16,
}

/// One enumeration of a tmux session: its windows, and the panes of each keyed
/// by window handle.
///
/// This is what both modes reconcile from: the daemon answers with the same
/// information the in-process client reads off the wire, so `sync_window` has
/// exactly one shape to consume (arch.md §7, "enumerated on touch").
pub(super) struct TmuxView {
    pub(super) windows: Vec<WindowLine>,
    pub(super) panes: HashMap<String, Vec<PaneLine>>,
}

impl TmuxView {
    /// Build a view from a `Structure` answer (one round trip for the windows,
    /// one per window for the panes — all on the daemon's resident client).
    pub(super) fn from_structure(
        structure: crate::service::daemon::protocol::TmuxStructure,
    ) -> Self {
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
    pub(super) async fn enumerate(control: &Arc<TmuxControl>, session: &str) -> Result<Self> {
        let lines = control
            .send(&crate::infra::tmux::list_windows_command(session))
            .await?;
        let windows: Vec<WindowLine> = lines
            .iter()
            .filter_map(|line| crate::infra::tmux::parse_window_line(line))
            .collect();

        let mut panes: HashMap<String, Vec<PaneLine>> = HashMap::new();
        for window in &windows {
            let lines = control
                .send(&crate::infra::tmux::list_panes_command(&window.handle))
                .await?;
            let rows: Vec<PaneLine> = lines
                .iter()
                .filter_map(|line| crate::infra::tmux::parse_pane_line(line))
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
pub(super) fn tmux_handle_from(lines: &[String], sigil: char) -> Option<String> {
    lines
        .iter()
        .map(|line| line.trim())
        .find(|line| line.starts_with(sigil) && line.len() > 1)
        .map(str::to_string)
}

/// The transport spec of a `Pipe` link. A `Control` link has none — its host,
/// socket and session live on the session — so asking one is a caller bug.
pub(super) fn pipe_transport(
    info: &crate::service::link_service::LinkInfo,
) -> Result<TransportConfig> {
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
pub(super) fn control_link_address(host: &TmuxHostConfig, tmux_argv: &[String]) -> LinkAddress {
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
    use crate::infra::tmux::{TmuxHostConfig, TmuxSocket};

    fn argv_of(host: &TmuxHostConfig) -> (Vec<String>, Vec<String>) {
        let prefix = crate::infra::tmux::host_prefix(host);
        let argv = crate::infra::tmux::argv(&prefix, &TmuxSocket::Default, "work");
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
