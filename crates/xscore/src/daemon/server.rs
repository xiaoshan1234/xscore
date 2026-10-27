// daemon/server.rs (v0.6.0 — resident daemon, ADR 0009/0010)
//
// `xscore daemon serve` — the process that owns what a CLI process
// cannot: live transports, tmux control clients and pane ring buffers.
//
// Why a single daemon for every link (ADR 0009 § 1): a pane may switch
// links at any time, and its buffer has to follow it. Buffers can only
// follow panes inside one process, so there is exactly one daemon and it
// holds all three maps:
//
//     links: HashMap<LinkId, Arc<dyn Transport>>
//     buffers: HashMap<BufferId, BufferSlot { link, bytes, pump }>
//     tmux:  HashMap<SessionId, ControlSlot { control, panes, alive }>
//
// `tmux` is the v0.6.0 addition (ADR 0010 §D2): a managed tmux session is
// driven by ONE `tmux -CC attach` client, and every Buffer's bytes arrive as
// `%output %N` on it. Holding that client here — instead of in whichever
// CLI invocation happened to attach — is what makes `data read` a *stream*
// (bytes since the daemon subscribed) rather than a screen snapshot.
//
// The CLI keeps the authoritative *metadata* (`state.json`: which pane is
// bound to which link). This process only owns *bytes*. That split is why
// `link add` still works with the daemon stopped, and why killing the
// daemon never corrupts the user's model — it just makes direct links
// report `connected: false` until something re-creates them.
//
// Lifecycle: bind (handling a stale socket file), accept, and exit on
//   * `Stop` (explicit `xscore daemon stop`),
//   * 5 s of idleness **with no links and no control slots** (so tests and
//     one-off sessions never leave a daemon behind; a daemon holding
//     either must stay up, because that connection is the whole point).
// The state machine is shared across platforms. Only the endpoint differs:
// a Unix socket on Unix, a named pipe on Windows.
#![cfg_attr(not(unix), allow(dead_code))]

use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncWrite};
#[cfg(windows)]
use tokio::net::windows::named_pipe::{NamedPipeServer, PipeMode, ServerOptions};
#[cfg(unix)]
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::Mutex;
use tokio::sync::Notify;
use tokio::task::JoinHandle;

use crate::infra::link::{factory as transport_factory, Transport};
use crate::infra::tmux::{self, TmuxControl, TmuxEvent};
use crate::infra::tmux::{TmuxHostConfig, TmuxSocket};
use crate::service::buffer_service::BufferId;
use crate::service::buffer_service::{RingBuffer, DIRECT_BUFFER_CAPACITY};
use crate::service::link_service::LinkId;
use crate::service::session_service::SessionId;
use crate::{Error, Result};

use super::client::socket_path;
use super::ipc;
use super::protocol::{
    self, BufferStatus, Request, Response, Status, TmuxPaneRow, TmuxStatus, TmuxStructure,
    TmuxWindowRow,
};

/// Same polling strategy as `local BufferRuntime` (v0.4.1): transport `read`
/// blocks until bytes arrive and transports may serialize read/write
/// behind one lock, so we cancel the read every 10 ms to give writers a
/// window. Serial's own read timeout (5 ms) is shorter and stays as is.
const READ_POLL: Duration = Duration::from_millis(10);

/// How long a `Read` waits for the pump to produce bytes before
/// reporting "nothing available" — identical to `local BufferRuntime::read`.
const READ_WAIT: Duration = Duration::from_millis(200);

/// Idle exit: no links, no control slots + no requests for this long ⇒ exit.
const IDLE_TIMEOUT: Duration = Duration::from_secs(5);
const IDLE_TICK: Duration = Duration::from_millis(100);

/// The idle window, overridable so a test does not have to wait 5 s to prove
/// that a foreground `daemon serve` ignores it. Not a user-facing setting: it
/// only ever shortens the window for the auto-started copy.
fn idle_timeout() -> Duration {
    std::env::var("XSCORE_IDLE_TIMEOUT_MS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .map(Duration::from_millis)
        .unwrap_or(IDLE_TIMEOUT)
}

struct BufferSlot {
    /// Currently attached link, if any. Cleared by Detach/DeleteLink —
    /// the buffer survives either way (that is the point of a pane).
    link: Option<LinkId>,
    buffer: Arc<Mutex<RingBuffer>>,
    /// The reader task feeding `buffer` from `link`'s transport.
    pump: Option<JoinHandle<()>>,
}

impl BufferSlot {
    fn new() -> Self {
        Self {
            link: None,
            buffer: Arc::new(Mutex::new(RingBuffer::new(DIRECT_BUFFER_CAPACITY))),
            pump: None,
        }
    }

    /// Stop the reader task. Never touches the link's transport (P0-a:
    /// closing a shared transport killed every other pane on the link).
    fn stop_pump(&mut self) {
        if let Some(handle) = self.pump.take() {
            handle.abort();
        }
    }
}

/// Everything that had to be true to spawn a session's control client, so the
/// daemon can re-spawn it after `%exit` (the ssh channel dropped, the server
/// was restarted) without the CLI having to say it again.
///
/// The daemon never reads `state.json` (ADR 0009 §2); these are the parameters
/// the CLI sent with `OpenTmux`, kept only for as long as the slot exists.
#[derive(Clone)]
struct ControlConfig {
    host: TmuxHostConfig,
    socket: TmuxSocket,
    name: String,
}

/// One pane of a tmux session: its `%N` address, its buffer, and the task
/// appending `%output %N` to it.
struct ControlPane {
    /// The xscore pane this `%N` belongs to. `Read`/`Write` arrive by pane id
    /// (that is all `state.json` knows), so the slot has to remember it.
    buffer: BufferId,
    buffer_data: Arc<Mutex<RingBuffer>>,
    /// `None` until `Attach` subscribes; aborted by `Detach`/`DeletePane`.
    pump: Option<JoinHandle<()>>,
}

impl ControlPane {
    fn stop_pump(&mut self) {
        if let Some(handle) = self.pump.take() {
            handle.abort();
        }
    }
}

/// One tmux session's control client — **one per session**, never one per
/// pane or per link (ADR 0010 §4, "one tmux session, one link").
///
/// A slot outlives its client: after `%exit` the buffers stay so the CLI can
/// still read what arrived (a pane is its buffer), and `OpenTmux`/`Structure`
/// re-spawn the control inside the same slot when the session comes back.
struct ControlSlot {
    control: Arc<TmuxControl>,
    /// `"%N"` → pane. Keyed by the address tmux demultiplexes on.
    panes: HashMap<String, ControlPane>,
    /// Shared with the exit watcher: false once tmux sent `%exit` (or the
    /// reader thread hit EOF), so `ProbeTmux` does not report a dead client.
    alive: Arc<AtomicBool>,
    config: ControlConfig,
    /// The task turning `%exit` into `alive = false`. Aborted on close.
    watcher: JoinHandle<()>,
}

impl ControlSlot {
    /// A client that tmux has hung up on is not "connected", even though the
    /// handle still exists (it is kept for its buffers).
    fn is_alive(&self) -> bool {
        self.alive.load(Ordering::SeqCst) && !self.control.is_closed()
    }

    fn stop_pumps(&mut self) {
        for pane in self.panes.values_mut() {
            pane.stop_pump();
        }
    }
}

struct DaemonState {
    links: Mutex<HashMap<LinkId, Arc<dyn Transport>>>,
    buffers: Mutex<HashMap<BufferId, BufferSlot>>,
    tmux: Mutex<HashMap<SessionId, ControlSlot>>,
    /// Serialises "look at a tmux slot, maybe replace its client, maybe add a
    /// subscriber". Two CLI invocations can hit the same session at once, and
    /// a second subscriber on one `%N` would append every byte twice.
    tmux_open: Mutex<()>,
    last_activity: Mutex<tokio::time::Instant>,
}

impl DaemonState {
    fn new() -> Self {
        Self {
            links: Mutex::new(HashMap::new()),
            buffers: Mutex::new(HashMap::new()),
            tmux: Mutex::new(HashMap::new()),
            tmux_open: Mutex::new(()),
            last_activity: Mutex::new(tokio::time::Instant::now()),
        }
    }

    async fn touch(&self) {
        *self.last_activity.lock().await = tokio::time::Instant::now();
    }

    /// Exit when nothing is left to serve. A daemon holding a link or a tmux
    /// control client must stay up — that connection is the entire reason it
    /// exists (and a control slot also holds the panes' buffers).
    async fn should_exit_idle(&self) -> bool {
        let idle = self.last_activity.lock().await.elapsed();
        idle_ready(
            !self.links.lock().await.is_empty(),
            !self.tmux.lock().await.is_empty(),
            idle,
            idle_timeout(),
        )
    }

    /// Abort every pump, close every transport and every control client
    /// (daemon shutdown — `daemon stop` is the only way to lose the buffers).
    async fn shutdown(&self) {
        {
            let mut panes = self.buffers.lock().await;
            for slot in panes.values_mut() {
                slot.stop_pump();
            }
        }
        let links: Vec<Arc<dyn Transport>> =
            self.links.lock().await.drain().map(|(_, t)| t).collect();
        for t in links {
            let _ = t.close().await;
        }
        let controls: Vec<ControlSlot> = self.tmux.lock().await.drain().map(|(_, c)| c).collect();
        for mut slot in controls {
            slot.stop_pumps();
            slot.watcher.abort();
            // `close()` drops stdin (tmux detaches) and reaps the child;
            // the buffers die with the slot, by design (arch.md §13).
            let _ = slot.control.close().await;
        }
    }

    async fn handle(&self, req: Request) -> Response {
        match self.try_handle(req).await {
            Ok(resp) => resp,
            Err(message) => Response::err(message),
        }
    }

    async fn try_handle(&self, req: Request) -> std::result::Result<Response, String> {
        match req {
            Request::Ping => Ok(Response::ok()),

            Request::Status => Ok(Response::status(self.status().await)),

            Request::CreateLink { link, transport } => {
                let created = create_transport(transport).await?;
                // Replace-if-present: re-creating a link is how a client
                // reconnects a transport the daemon lost (e.g. after a
                // daemon restart), so it must be idempotent.
                let old = self.links.lock().await.insert(link, created);
                if let Some(old) = old {
                    let _ = old.close().await;
                }
                Ok(Response::ok())
            }

            Request::DeleteLink { link } => {
                let transport = self.links.lock().await.remove(&link);
                {
                    // Unbind this link's panes but KEEP their slots and
                    // buffers: the user may attach them again, and a
                    // pane's history is not the link's property.
                    let mut panes = self.buffers.lock().await;
                    for slot in panes.values_mut() {
                        if slot.link == Some(link) {
                            slot.stop_pump();
                            slot.link = None;
                        }
                    }
                }
                if let Some(t) = transport {
                    let _ = t.close().await;
                }
                Ok(Response::ok())
            }

            Request::CreateBuffer { buffer } => {
                self.buffers
                    .lock()
                    .await
                    .entry(buffer)
                    .or_insert_with(BufferSlot::new);
                Ok(Response::ok())
            }

            Request::DeleteBuffer { buffer } => {
                if let Some(mut slot) = self.buffers.lock().await.remove(&buffer) {
                    slot.stop_pump();
                }
                // A tmux pane's buffer lives in its session's slot; `pane del`
                // is a full teardown, so stop the subscriber and drop it.
                let mut tmux = self.tmux.lock().await;
                for slot in tmux.values_mut() {
                    let keys: Vec<String> = slot
                        .panes
                        .iter()
                        .filter(|(_, cp)| cp.buffer == buffer)
                        .map(|(k, _)| k.clone())
                        .collect();
                    for key in keys {
                        if let Some(mut cp) = slot.panes.remove(&key) {
                            cp.stop_pump();
                        }
                    }
                }
                Ok(Response::ok())
            }

            Request::OpenTmux {
                session,
                host,
                socket,
                name,
                cols: _,
                rows: _,
            } => {
                // `cols`/`rows` are informational: `-C attach` takes no size,
                // and a fresh client adopts the session's own geometry.
                let config = ControlConfig { host, socket, name };
                self.open_control(session, config).await?;
                Ok(Response::structure(self.structure(session).await?))
            }

            Request::Structure { session } => {
                // The slot's own config is enough to re-open a client that
                // died; without a slot the daemon has never been told where
                // this session lives (it never reads `state.json`).
                let config = {
                    let slots = self.tmux.lock().await;
                    slots.get(&session).map(|slot| slot.config.clone())
                };
                match config {
                    Some(config) => {
                        self.open_control(session, config).await?;
                        Ok(Response::structure(self.structure(session).await?))
                    }
                    None => Err(format!(
                        "session {session} has no control client in the daemon (open it first)"
                    )),
                }
            }

            Request::TmuxCommand { session, command } => {
                let control = self.control_for(session).await?;
                let lines = control.send(&command).await.map_err(|e| e.to_string())?;
                Ok(Response::lines(lines))
            }

            Request::CloseTmux { session } => {
                self.close_control(session).await;
                Ok(Response::ok())
            }

            Request::ProbeTmux { session } => Ok(Response::bool(self.probe_control(session).await)),

            Request::Attach {
                buffer,
                link,
                key,
                session,
            } => match (key, session) {
                (Some(key), Some(session)) => {
                    self.attach_control_buffer(session, buffer, key).await?;
                    Ok(Response::ok())
                }
                (None, _) => {
                    let link = link.ok_or_else(|| {
                        format!("buffer {buffer} has no link and no `%N` address to attach to")
                    })?;
                    self.attach_raw_buffer(buffer, link).await
                }
                (Some(_), None) => Err(format!(
                    "buffer {buffer} was given a `%N` key without the session it belongs to"
                )),
            },

            Request::Detach { buffer } => {
                if let Some(slot) = self.buffers.lock().await.get_mut(&buffer) {
                    slot.stop_pump();
                    slot.link = None;
                }
                // A tmux pane cannot be detached (its `%N` is its identity on
                // the server), but stopping its subscriber is the closest
                // honest answer if someone asks.
                for slot in self.tmux.lock().await.values_mut() {
                    let keys: Vec<String> = slot
                        .panes
                        .iter()
                        .filter(|(_, cp)| cp.buffer == buffer)
                        .map(|(k, _)| k.clone())
                        .collect();
                    for key in keys {
                        if let Some(cp) = slot.panes.get_mut(&key) {
                            cp.stop_pump();
                        }
                    }
                }
                // Idempotent: detaching an unknown/already-detached pane
                // is the state the caller asked for.
                Ok(Response::ok())
            }

            Request::Read { buffer, max } => {
                let buffer = {
                    let native = self
                        .buffers
                        .lock()
                        .await
                        .get(&buffer)
                        .map(|s| Arc::clone(&s.buffer));
                    match native {
                        Some(buffer) => buffer,
                        None => self
                            .control_pane_buffer(buffer)
                            .await
                            .ok_or_else(|| format!("buffer {buffer} has no slot in the daemon"))?,
                    }
                };
                Ok(Response::bytes(&read_snapshot(buffer, max).await))
            }

            Request::Write { buffer, data } => {
                let bytes = protocol::from_hex(&data).map_err(|e| e.to_string())?;
                // A tmux pane's write is one protocol line on its session's
                // control client, not a byte stream: same text rules as
                // `control BufferRuntime` (UTF-8, no NUL — the protocol is line-framed).
                if let Some((control, key)) = self.control_target(buffer).await {
                    let text = std::str::from_utf8(&bytes).map_err(|_| {
                        "a tmux pane takes UTF-8 text; this payload is not valid UTF-8".to_string()
                    })?;
                    let command = tmux::send_keys_literal(&key, text).map_err(|e| e.to_string())?;
                    control.send(&command).await.map_err(|e| e.to_string())?;
                    return Ok(Response::count(bytes.len()));
                }
                let transport = self.buffer_transport(buffer).await?;
                let n = transport.write(&bytes).await.map_err(|e| e.to_string())?;
                Ok(Response::count(n))
            }

            Request::Resize { buffer, cols, rows } => {
                if let Some((control, key)) = self.control_target(buffer).await {
                    control
                        .send(&tmux::resize_pane_command(&key, cols, rows))
                        .await
                        .map_err(|e| e.to_string())?;
                    return Ok(Response::ok());
                }
                let transport = self.buffer_transport(buffer).await?;
                transport
                    .resize(cols, rows)
                    .await
                    .map_err(|e| e.to_string())?;
                Ok(Response::ok())
            }

            Request::Stop => Ok(Response::ok()),
        }
    }

    /// The native pipe path of `Attach`: one pump on one transport, Q1
    /// enforced (a direct link has exactly ONE active pane).
    async fn attach_raw_buffer(
        &self,
        pane: BufferId,
        link: LinkId,
    ) -> std::result::Result<Response, String> {
        // Never hold both locks: `links` is cloned out first so
        // the ordering (links → panes) can't invert anywhere.
        let transport = self
            .links
            .lock()
            .await
            .get(&link)
            .cloned()
            .ok_or_else(|| format!("link {link} is not held by the daemon"))?;

        let mut panes = self.buffers.lock().await;
        // Q1: a direct link has exactly ONE active pane. Mirror
        // the manager's rule here — the daemon must enforce it
        // too, otherwise two pumps would race on one transport
        // (and duplicate every byte into two buffers).
        for (other, slot) in panes.iter_mut() {
            if *other != pane && slot.link == Some(link) {
                slot.stop_pump();
                slot.link = None;
            }
        }
        let slot = panes.entry(pane).or_insert_with(BufferSlot::new);
        if slot.link == Some(link) && slot.pump.is_some() {
            return Ok(Response::ok());
        }
        slot.stop_pump();
        slot.link = Some(link);
        slot.pump = Some(spawn_pump(transport, Arc::clone(&slot.buffer)));
        Ok(Response::ok())
    }

    // ----- tmux control slots -----

    /// Get the session's control client, spawning (or re-spawning) it as
    /// needed. Idempotent: a live slot is reused as it is.
    ///
    /// `tmux_open` is held across the check-and-spawn so two CLI invocations
    /// cannot each start a client for the same session (two clients would be
    /// two `%output` streams, i.e. every byte twice).
    async fn open_control(
        &self,
        session: SessionId,
        config: ControlConfig,
    ) -> std::result::Result<Arc<TmuxControl>, String> {
        let _guard = self.tmux_open.lock().await;
        let mut slots = self.tmux.lock().await;
        if let Some(slot) = slots.get_mut(&session) {
            if slot.is_alive() {
                slot.config = config;
                return Ok(Arc::clone(&slot.control));
            }
            // The client is gone (`%exit`, ssh dropped). Replace it, but KEEP
            // the panes and their buffers: they are the only history there is
            // (tmux replays nothing), and a re-attach re-subscribes them.
            slot.stop_pumps();
            slot.watcher.abort();
            let control = spawn_control(&config)?;
            // A fresh flag per client: the old watcher may still store `false`
            // as it winds down, and it must not mark the new client dead.
            let alive = Arc::new(AtomicBool::new(true));
            slot.watcher = spawn_exit_watcher(Arc::clone(&control), Arc::clone(&alive));
            slot.control = Arc::clone(&control);
            slot.alive = alive;
            slot.config = config;
            return Ok(control);
        }
        let control = spawn_control(&config)?;
        let alive = Arc::new(AtomicBool::new(true));
        let watcher = spawn_exit_watcher(Arc::clone(&control), Arc::clone(&alive));
        slots.insert(
            session,
            ControlSlot {
                control: Arc::clone(&control),
                panes: HashMap::new(),
                alive,
                config,
                watcher,
            },
        );
        Ok(control)
    }

    /// The session's live control client, or an error naming why not.
    async fn control_for(
        &self,
        session: SessionId,
    ) -> std::result::Result<Arc<TmuxControl>, String> {
        let slots = self.tmux.lock().await;
        let slot = slots
            .get(&session)
            .ok_or_else(|| format!("session {session} has no control client in the daemon"))?;
        if !slot.is_alive() {
            return Err(format!(
                "session {session}'s control client is not alive (re-open it)"
            ));
        }
        Ok(Arc::clone(&slot.control))
    }

    /// Enumerate the session through its control client: `list-windows` once,
    /// then `list-panes` per window. One round trip each — no `tmux` exec per
    /// command (arch.md §6).
    async fn structure(&self, session: SessionId) -> std::result::Result<TmuxStructure, String> {
        let (control, name) = {
            let slots = self.tmux.lock().await;
            let slot = slots
                .get(&session)
                .ok_or_else(|| format!("session {session} has no control client in the daemon"))?;
            (Arc::clone(&slot.control), slot.config.name.clone())
        };

        let window_lines = control
            .send(&tmux::list_windows_command(&name))
            .await
            .map_err(|e| e.to_string())?;
        let mut out = TmuxStructure::default();
        for line in &window_lines {
            let Some(window) = tmux::parse_window_line(line) else {
                continue;
            };
            let pane_lines = control
                .send(&tmux::list_panes_command(&window.handle))
                .await
                .map_err(|e| e.to_string())?;
            for pane_line in &pane_lines {
                let Some(pane) = tmux::parse_pane_line(pane_line) else {
                    continue;
                };
                out.panes.push(TmuxPaneRow {
                    handle: pane.handle,
                    cols: pane.cols,
                    rows: pane.rows,
                    x: pane.x,
                    y: pane.y,
                    window_handle: window.handle.clone(),
                });
            }
            out.windows.push(TmuxWindowRow {
                handle: window.handle,
                name: window.name,
                cols: window.cols,
                rows: window.rows,
            });
        }
        Ok(out)
    }

    async fn probe_control(&self, session: SessionId) -> bool {
        self.tmux
            .lock()
            .await
            .get(&session)
            .map(ControlSlot::is_alive)
            .unwrap_or(false)
    }

    /// Drop the slot — and therefore the child (`TmuxControl::drop` kills it)
    /// and every buffer it held.
    async fn close_control(&self, session: SessionId) {
        let slot = self.tmux.lock().await.remove(&session);
        if let Some(mut slot) = slot {
            slot.stop_pumps();
            slot.watcher.abort();
            let _ = slot.control.close().await;
        }
    }

    /// Subscribe this pane's `%output %N` into its buffer.
    ///
    /// Idempotent: an existing subscriber for that `%N` is left alone (a
    /// second one would append every byte twice). The pane id is recorded so
    /// `Read`/`Write` — which only know the xscore pane — can find it.
    async fn attach_control_buffer(
        &self,
        session: SessionId,
        pane: BufferId,
        key: String,
    ) -> std::result::Result<(), String> {
        let _guard = self.tmux_open.lock().await;
        {
            let mut slots = self.tmux.lock().await;
            let slot = slots
                .get_mut(&session)
                .ok_or_else(|| format!("session {session} has no control client in the daemon"))?;
            if !slot.is_alive() {
                return Err(format!(
                    "session {session}'s control client is not alive (re-open it)"
                ));
            }
            if let Some(existing) = slot.panes.get_mut(&key) {
                existing.buffer = pane;
                if existing.pump.is_some() {
                    return Ok(());
                }
            }
        }

        // Subscribe BEFORE taking the slot lock again: `output` only registers
        // a channel, and the pump below must not miss it.
        let control = {
            let slots = self.tmux.lock().await;
            let slot = slots
                .get(&session)
                .ok_or_else(|| format!("session {session} has no control client in the daemon"))?;
            Arc::clone(&slot.control)
        };
        let mut rx = control.output(&key).await.map_err(|e| e.to_string())?;

        let mut slots = self.tmux.lock().await;
        let slot = slots
            .get_mut(&session)
            .ok_or_else(|| format!("session {session} has no control client in the daemon"))?;
        let buffer = slot
            .panes
            .get(&key)
            .map(|cp| Arc::clone(&cp.buffer_data))
            .unwrap_or_else(|| Arc::new(Mutex::new(RingBuffer::new(DIRECT_BUFFER_CAPACITY))));
        let sink = Arc::clone(&buffer);
        let pump = tokio::spawn(async move {
            // Ends when the client closes: `%exit`/EOF drops every sender.
            while let Some(bytes) = rx.recv().await {
                sink.lock().await.push_slice(&bytes);
            }
        });
        slot.panes.insert(
            key,
            ControlPane {
                buffer: pane,
                buffer_data: buffer,
                pump: Some(pump),
            },
        );
        Ok(())
    }

    /// The buffer of the tmux pane `pane`, if any slot is serving it.
    async fn control_pane_buffer(&self, pane: BufferId) -> Option<Arc<Mutex<RingBuffer>>> {
        let slots = self.tmux.lock().await;
        slots
            .values()
            .flat_map(|slot| slot.panes.values())
            .find(|cp| cp.buffer == pane)
            .map(|cp| Arc::clone(&cp.buffer_data))
    }

    /// The control client and `%N` behind a tmux pane: what a write or a
    /// resize must address.
    async fn control_target(&self, pane: BufferId) -> Option<(Arc<TmuxControl>, String)> {
        let slots = self.tmux.lock().await;
        for slot in slots.values() {
            if !slot.is_alive() {
                continue;
            }
            if let Some((key, _)) = slot.panes.iter().find(|(_, cp)| cp.buffer == pane) {
                return Some((Arc::clone(&slot.control), key.clone()));
            }
        }
        None
    }

    /// The transport behind a pane's current link. The error text mirrors
    /// `Error::NotAttached` so the CLI's message stays the same as
    /// it was when the runtime lived in-process.
    async fn buffer_transport(
        &self,
        pane: BufferId,
    ) -> std::result::Result<Arc<dyn Transport>, String> {
        let link = {
            let panes = self.buffers.lock().await;
            match panes.get(&pane) {
                Some(slot) => slot.link,
                None => None,
            }
        };
        let link = link.ok_or_else(|| {
            format!(
                "pane {pane} is not attached to any link (orphan — attach via \
                 `xscore pane attach --pane {pane} --link <lid>`)"
            )
        })?;
        self.links
            .lock()
            .await
            .get(&link)
            .cloned()
            .ok_or_else(|| format!("link {link} is not held by the daemon"))
    }

    async fn status(&self) -> Status {
        let mut links: Vec<LinkId> = self.links.lock().await.keys().copied().collect();
        links.sort_by_key(|l| l.to_string());

        let mut buffers = Vec::new();
        for (buffer_id, slot) in self.buffers.lock().await.iter() {
            buffers.push(BufferStatus {
                buffer: *buffer_id,
                link: slot.link.map(|l| l.to_string()).unwrap_or_default(),
                buffered: slot.buffer.lock().await.len(),
            });
        }
        buffers.sort_by_key(|buffer| buffer.buffer.to_string());

        let mut tmux = Vec::new();
        for (session, slot) in self.tmux.lock().await.iter() {
            let mut buffered = 0usize;
            let mut attached = 0usize;
            for cp in slot.panes.values() {
                if cp.pump.is_some() {
                    attached += 1;
                }
                buffered += cp.buffer_data.lock().await.len();
            }
            tmux.push(TmuxStatus {
                session: *session,
                name: slot.config.name.clone(),
                alive: slot.is_alive(),
                panes: attached,
                buffered,
            });
        }
        tmux.sort_by_key(|t| t.name.clone());

        Status {
            pid: std::process::id(),
            links,
            buffers,
            tmux,
        }
    }
}

/// Build + open a transport for a link. The CLI opens a pipe link's transport
/// in `add_link_in`; the daemon owns its own transports, so it opens them here —
/// without it `ShellTransport::read/write` fail with "not open".
async fn create_transport(
    config: crate::infra::link::TransportConfig,
) -> std::result::Result<Arc<dyn Transport>, String> {
    let mut transport = transport_factory::transport_for(config)
        .await
        .map_err(|e| e.to_string())?;
    transport.open().await.map_err(|e| e.to_string())?;
    Ok(Arc::from(transport))
}

fn spawn_pump(transport: Arc<dyn Transport>, buffer: Arc<Mutex<RingBuffer>>) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut buf = [0u8; 4096];
        loop {
            match tokio::time::timeout(READ_POLL, transport.read(&mut buf)).await {
                Ok(Ok(0)) => tokio::time::sleep(READ_POLL).await,
                Ok(Ok(n)) => buffer.lock().await.push_slice(&buf[..n]),
                // Transport error (child exited, ssh dropped): the pump's
                // job is done. The buffer keeps what it already has.
                Ok(Err(_)) => break,
                // Timed out: no bytes yet. Re-arm; the cancel released
                // the transport lock so writers get their window.
                Err(_) => {}
            }
        }
    })
}

/// Wait up to `READ_WAIT` for the buffer to hold bytes, then copy out a
/// **non-destructive** snapshot (`local BufferRuntime::read`'s contract, which the
/// CLI relies on: reading twice returns the same bytes).
async fn read_snapshot(buffer: Arc<Mutex<RingBuffer>>, max: usize) -> Vec<u8> {
    let max = max.max(1);
    let deadline = tokio::time::Instant::now() + READ_WAIT;
    loop {
        {
            let ring = buffer.lock().await;
            if !ring.is_empty() {
                let mut out = vec![0u8; max.min(ring.len())];
                let n = ring.copy_out(&mut out);
                out.truncate(n);
                return out;
            }
        }
        if tokio::time::Instant::now() >= deadline {
            return Vec::new();
        }
        tokio::time::sleep(READ_POLL).await;
    }
}

/// Spawn `tmux -CC attach` for a session. The client — pipes, reader thread,
/// writer thread — is exactly the one the CLI used to own; only its home
/// changed (ADR 0010 §D2).
fn spawn_control(config: &ControlConfig) -> std::result::Result<Arc<TmuxControl>, String> {
    let prefix = tmux::host_prefix(&config.host);
    TmuxControl::spawn(&prefix, &config.socket, &config.name)
        .map(Arc::new)
        .map_err(|e| e.to_string())
}

/// Turn `%exit` — tmux hanging up, the ssh channel dropping, the server
/// going away — into `alive = false`, so `ProbeTmux` stops reporting a dead
/// client as connected. The buffers stay: they are the only history there is.
///
/// The pane pumps need no help here: `%exit` makes `TmuxControl` drop every
/// `%output` sender, which ends their `recv` loops.
fn spawn_exit_watcher(control: Arc<TmuxControl>, alive: Arc<AtomicBool>) -> JoinHandle<()> {
    let mut events = control.subscribe();
    tokio::spawn(async move {
        while let Some(event) = events.recv().await {
            if let TmuxEvent::Exit { reason } = &event {
                tracing::info!("tmux: control client exited: {reason}");
                break;
            }
        }
        alive.store(false, Ordering::SeqCst);
    })
}

/// Exit only when nothing is left to serve: no native links, no tmux control
/// slots, and `timeout` of quiet. Split out so the gate itself is testable
/// without spawning a client. Note that *whether* the gate applies at all is
/// the caller's decision: only the auto-started daemon passes `--idle-exit`.
fn idle_ready(has_links: bool, has_tmux: bool, idle: Duration, timeout: Duration) -> bool {
    !has_links && !has_tmux && idle >= timeout
}

/// Bind the socket and serve until told to stop.
///
/// Startup is single-instance via a **lock file** (`daemon.lock`, created
/// with `create_new` — atomic) taken *before* the socket is bound. Two
/// concurrent CLI processes both auto-starting the daemon used to be able
/// to race: whichever bound second would `unlink` the winner's socket
/// file and bind fresh, leaving the winner listening on an orphaned inode
/// and both processes holding links forever. The lock makes "who binds"
/// a single winner, and a crashed daemon's lock is detected and taken
/// over (its socket no longer answers).
#[cfg(unix)]
pub async fn run(idle_exit: bool) -> Result<()> {
    // Ignore SIGHUP: the daemon may have been started by a `link add`
    // that ran in a terminal which is since closed. Dropping every live
    // direct link because a terminal went away is never what the user
    // wants — links are only torn down explicitly (`link del`) or when
    // the daemon is idle. SIGINT/SIGTERM are handled further down, through
    // the same shutdown path as `daemon stop`.
    {
        use tokio::signal::unix::{signal, SignalKind};
        if let Ok(mut hup) = signal(SignalKind::hangup()) {
            tokio::spawn(async move {
                while hup.recv().await.is_some() {
                    // Swallow it. Deliberately no logging: stderr is null
                    // in the auto-started case.
                }
            });
        }
    }

    let path = socket_path();

    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)
            .map_err(|e| Error::Daemon(format!("mkdir {}: {e}", dir.display())))?;
        // The socket's directory is private by construction (0700 is the
        // convention for XDG_RUNTIME_DIR-style dirs); be explicit anyway
        // because we are about to expose a control channel in it.
        restrict_dir(dir);
    }

    let lock_path = path.with_extension("lock");
    if !acquire_startup_lock(&lock_path, &path).await? {
        eprintln!("xscore daemon: already running at {}", path.display());
        return Ok(());
    }

    if Path::new(&path).exists() {
        // Nobody else can be starting (we hold the lock), so a failed
        // connect means the file is a leftover from a crashed daemon.
        match UnixStream::connect(&path).await {
            Ok(_) => {
                eprintln!("xscore daemon: already running at {}", path.display());
                let _ = std::fs::remove_file(&lock_path);
                return Ok(());
            }
            Err(_) => {
                let _ = std::fs::remove_file(&path);
            }
        }
    }

    let listener = match UnixListener::bind(&path) {
        Ok(l) => l,
        Err(e) => {
            // Do not leave a lock we don't serve.
            let _ = std::fs::remove_file(&lock_path);
            return Err(Error::Daemon(format!("bind {}: {e}", path.display())));
        }
    };
    tracing::info!("xscore daemon listening on {}", path.display());

    let state = Arc::new(DaemonState::new());
    let shutdown = Arc::new(Notify::new());

    // Ctrl-C and SIGTERM on a foreground `serve` wind down through the same
    // path as `daemon stop`, so the socket and lock are removed instead of
    // left behind for the next start to detect and unlink.
    {
        let sd = Arc::clone(&shutdown);
        tokio::spawn(async move {
            use tokio::signal::unix::{signal, SignalKind};
            let mut term = match signal(SignalKind::terminate()) {
                Ok(s) => s,
                Err(_) => return,
            };
            tokio::select! {
                _ = tokio::signal::ctrl_c() => {}
                _ = term.recv() => {}
            }
            sd.notify_one();
        });
    }

    loop {
        tokio::select! {
            accepted = listener.accept() => {
                match accepted {
                    Ok((stream, _)) => {
                        let st = Arc::clone(&state);
                        let sd = Arc::clone(&shutdown);
                        tokio::spawn(async move { serve_conn(stream, st, sd).await });
                    }
                    Err(e) => tracing::warn!("daemon accept failed: {e}"),
                }
            }
            _ = shutdown.notified() => break,
            // Only the auto-started daemon leaves on its own. A foreground
            // `xscore daemon serve` is an explicit "stay up": it waits for
            // `daemon stop`, Ctrl-C or SIGTERM rather than exiting after 5 s
            // with nothing to supervise, which looks exactly like a crash.
            _ = tokio::time::sleep(IDLE_TICK), if idle_exit => {
                if state.should_exit_idle().await {
                    tracing::info!("daemon exiting: idle with no links and no tmux clients");
                    break;
                }
            }
        }
    }

    state.shutdown().await;
    // Best effort: the socket is ours to clean up, and leaving it behind
    // would only be a stale file for the next start to remove anyway.
    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_file(&lock_path);
    Ok(())
}

#[cfg(windows)]
fn next_pipe(name: &str, first: bool) -> Result<NamedPipeServer> {
    let mut options = ServerOptions::new();
    options
        .pipe_mode(PipeMode::Byte)
        .max_instances(32)
        .first_pipe_instance(first);
    options
        .create(name)
        .map_err(|e| Error::Daemon(format!("create named pipe {name}: {e}")))
}

/// Windows uses one named-pipe instance per client. The daemon creates the
/// next instance immediately after accepting one, so a concurrent CLI never
/// sees a gap while another request is being handled.
#[cfg(windows)]
pub async fn run(idle_exit: bool) -> Result<()> {
    let path = socket_path();
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)
            .map_err(|e| Error::Daemon(format!("mkdir {}: {e}", dir.display())))?;
    }

    // An endpoint can still be alive after its lock file was removed. Check it
    // first so two auto-started daemons never fight over `first_pipe_instance`.
    if ipc::probe(&path).await {
        return Ok(());
    }

    let lock_path = path.with_extension("lock");
    if !acquire_startup_lock(&lock_path, &path).await? {
        return Ok(());
    }

    let name = ipc::display_address(&path);
    let mut server = match next_pipe(&name, true) {
        Ok(server) => server,
        Err(e) => {
            let _ = std::fs::remove_file(&lock_path);
            return Err(e);
        }
    };
    tracing::info!("xscore daemon listening on {name}");

    let state = Arc::new(DaemonState::new());
    let shutdown = Arc::new(Notify::new());
    {
        let sd = Arc::clone(&shutdown);
        tokio::spawn(async move {
            let _ = tokio::signal::ctrl_c().await;
            sd.notify_one();
        });
    }

    loop {
        tokio::select! {
            connected = server.connect() => {
                if let Err(e) = connected {
                    tracing::warn!("daemon accept failed: {e}");
                    continue;
                }
                let stream = server;
                server = match next_pipe(&name, false) {
                    Ok(next) => next,
                    Err(e) => {
                        tracing::error!("daemon could not create next pipe instance: {e}");
                        break;
                    }
                };
                let st = Arc::clone(&state);
                let sd = Arc::clone(&shutdown);
                tokio::spawn(async move { serve_conn(stream, st, sd).await });
            }
            _ = shutdown.notified() => break,
            _ = tokio::time::sleep(IDLE_TICK), if idle_exit => {
                if state.should_exit_idle().await {
                    tracing::info!("daemon exiting: idle with no links and no tmux clients");
                    break;
                }
            }
        }
    }

    state.shutdown().await;
    let _ = std::fs::remove_file(&lock_path);
    Ok(())
}

#[cfg(all(not(unix), not(windows)))]
pub async fn run(_idle_exit: bool) -> Result<()> {
    Err(Error::Daemon(DISCLAIMER.to_string()))
}

/// The message used by targets with neither Unix sockets nor named pipes.
#[cfg(all(not(unix), not(windows)))]
pub(crate) const DISCLAIMER: &str =
    "the xscore daemon requires Unix sockets or Windows named pipes on this target";

/// Take the single-instance lock. `Ok(false)` means a live daemon owns it
/// (this process should just exit); `Ok(true)` means we may bind.
///
/// A lock whose socket does not answer is stale — the owner crashed, or
/// died between taking the lock and binding. We wait briefly first so a
/// daemon that is *just* binding isn't mistaken for a corpse.
async fn acquire_startup_lock(lock_path: &Path, socket: &Path) -> Result<bool> {
    for _ in 0..3 {
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(lock_path)
        {
            Ok(_) => return Ok(true),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                for _ in 0..5 {
                    if ipc::probe(socket).await {
                        return Ok(false);
                    }
                    tokio::time::sleep(Duration::from_millis(20)).await;
                }
                // Stale: take it over. If another process beats us to it,
                // its `create_new` wins and ours reports AlreadyExists on
                // the next pass, so at most one daemon ends up serving.
                let _ = std::fs::remove_file(lock_path);
            }
            Err(e) => return Err(Error::Daemon(format!("lock {}: {e}", lock_path.display()))),
        }
    }
    Ok(false)
}

/// One connection: read requests until EOF. The CLI opens one connection
/// per request, but looping keeps a long-lived client (a future TUI) from
/// needing protocol changes.
async fn serve_conn<S>(mut stream: S, state: Arc<DaemonState>, shutdown: Arc<Notify>)
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    loop {
        let req: Request = match protocol::read_frame(&mut stream).await {
            Ok(Some(req)) => req,
            Ok(None) => return,
            Err(e) => {
                tracing::warn!("daemon: bad frame: {e}");
                return;
            }
        };
        state.touch().await;
        let stop = matches!(req, Request::Stop);
        let resp = state.handle(req).await;
        if protocol::write_frame(&mut stream, &resp).await.is_err() {
            return;
        }
        if stop {
            // Answer first, then let the accept loop wind down.
            shutdown.notify_one();
            return;
        }
    }
}

#[cfg(unix)]
fn restrict_dir(dir: &Path) {
    use std::os::unix::fs::PermissionsExt;
    if let Ok(meta) = std::fs::metadata(dir) {
        let mut perms = meta.permissions();
        // rwx------ : the socket is a control channel for this user only.
        perms.set_mode(0o700);
        let _ = std::fs::set_permissions(dir, perms);
    }
}

#[cfg(not(unix))]
fn restrict_dir(_dir: &Path) {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::infra::link::LinkAddress;
    use async_trait::async_trait;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// A scripted transport: `read` returns queued chunks (then blocks
    /// like a real transport), `write` records bytes.
    #[derive(Debug)]
    struct Fake {
        chunks: Mutex<Vec<Vec<u8>>>,
        written: Mutex<Vec<u8>>,
        closed: AtomicUsize,
    }

    impl Fake {
        fn new(chunks: Vec<&[u8]>) -> Arc<Self> {
            Arc::new(Self {
                chunks: Mutex::new(chunks.into_iter().map(|c| c.to_vec()).collect()),
                written: Mutex::new(Vec::new()),
                closed: AtomicUsize::new(0),
            })
        }
    }

    #[async_trait]
    impl Transport for Fake {
        fn kind(&self) -> &'static str {
            "fake"
        }
        fn address(&self) -> LinkAddress {
            LinkAddress::Shell {
                command: String::new(),
                args: vec![],
            }
        }
        async fn open(&mut self) -> Result<()> {
            Ok(())
        }
        async fn write(&self, data: &[u8]) -> Result<usize> {
            self.written.lock().await.extend_from_slice(data);
            Ok(data.len())
        }
        async fn read(&self, buf: &mut [u8]) -> Result<usize> {
            // Model a blocking transport: hold the "lock" a moment when
            // there is nothing queued, so the pump's timeout path runs.
            let mut chunks = self.chunks.lock().await;
            match chunks.pop() {
                Some(chunk) => {
                    let n = chunk.len().min(buf.len());
                    buf[..n].copy_from_slice(&chunk[..n]);
                    Ok(n)
                }
                None => {
                    drop(chunks);
                    tokio::time::sleep(Duration::from_millis(50)).await;
                    Ok(0)
                }
            }
        }
        async fn resize(&self, _: u16, _: u16) -> Result<()> {
            Ok(())
        }
        async fn close(&self) -> Result<()> {
            self.closed.fetch_add(1, Ordering::Relaxed);
            Ok(())
        }
    }

    fn state_with_link(link: LinkId, fake: Arc<Fake>) -> DaemonState {
        let st = DaemonState::new();
        let t: Arc<dyn Transport> = fake;
        // Synchronous insert: no runtime needed for the test's setup.
        st.links.try_lock().expect("fresh lock").insert(link, t);
        st
    }

    /// The native `Attach` shape (a `%N` key is the tmux one).
    fn attach(pane: BufferId, link: LinkId) -> Request {
        Request::Attach {
            buffer: pane,
            link: Some(link),
            key: None,
            session: None,
        }
    }

    async fn read_pane(st: &DaemonState, pane: BufferId, max: usize) -> Vec<u8> {
        let resp = st.handle(Request::Read { buffer: pane, max }).await;
        assert!(resp.ok, "read failed: {:?}", resp.error);
        protocol::from_hex(resp.data.as_deref().unwrap_or("")).unwrap()
    }

    #[tokio::test]
    async fn attach_pumps_bytes_into_the_pane_buffer() {
        let link = LinkId::new();
        let pane = BufferId::new();
        let fake = Fake::new(vec![b"hello\n"]);
        let st = state_with_link(link, Arc::clone(&fake));

        st.handle(Request::CreateBuffer { buffer: pane }).await;
        assert!(st.handle(attach(pane, link)).await.ok);

        // The pump drains the transport into the buffer; Read waits for
        // it (up to READ_WAIT) instead of racing it.
        let bytes = read_pane(&st, pane, 4096).await;
        assert_eq!(bytes, b"hello\n");
        // Snapshot semantics: reading twice must not consume.
        assert_eq!(read_pane(&st, pane, 4096).await, b"hello\n");
    }

    #[tokio::test]
    async fn write_goes_to_the_attached_transport() {
        let link = LinkId::new();
        let pane = BufferId::new();
        let fake = Fake::new(vec![]);
        let st = state_with_link(link, Arc::clone(&fake));
        st.handle(Request::CreateBuffer { buffer: pane }).await;
        st.handle(attach(pane, link)).await;

        let resp = st
            .handle(Request::Write {
                buffer: pane,
                data: protocol::to_hex(b"abc"),
            })
            .await;
        assert!(resp.ok, "{:?}", resp.error);
        assert_eq!(resp.n, Some(3));
        assert_eq!(&*fake.written.lock().await, b"abc");
    }

    #[tokio::test]
    async fn write_without_an_attached_link_is_refused() {
        let pane = BufferId::new();
        let st = DaemonState::new();
        st.handle(Request::CreateBuffer { buffer: pane }).await;
        let resp = st
            .handle(Request::Write {
                buffer: pane,
                data: protocol::to_hex(b"x"),
            })
            .await;
        assert!(!resp.ok);
        let err = resp.error.unwrap_or_default();
        // Same wording the in-process runtime produced (NotAttached).
        assert!(err.contains("not attached"), "got: {err}");
    }

    #[tokio::test]
    async fn detach_keeps_the_buffer_and_stops_the_pump() {
        let link = LinkId::new();
        let pane = BufferId::new();
        let fake = Fake::new(vec![b"abc"]);
        let st = state_with_link(link, fake);
        st.handle(Request::CreateBuffer { buffer: pane }).await;
        st.handle(attach(pane, link)).await;
        assert_eq!(read_pane(&st, pane, 16).await, b"abc");

        assert!(st.handle(Request::Detach { buffer: pane }).await.ok);
        // Buffer survives the detach ...
        assert_eq!(read_pane(&st, pane, 16).await, b"abc");
        // ... but the pane is no longer writable.
        let resp = st
            .handle(Request::Write {
                buffer: pane,
                data: protocol::to_hex(b"z"),
            })
            .await;
        assert!(!resp.ok);
    }

    #[tokio::test]
    async fn attach_auto_detaches_the_previous_pane_q1() {
        let link = LinkId::new();
        let p1 = BufferId::new();
        let p2 = BufferId::new();
        let fake = Fake::new(vec![]);
        let st = state_with_link(link, fake);
        for p in [p1, p2] {
            st.handle(Request::CreateBuffer { buffer: p }).await;
        }
        st.handle(attach(p1, link)).await;
        st.handle(attach(p2, link)).await;

        let status = st.status().await;
        let attached: Vec<BufferId> = status
            .buffers
            .iter()
            .filter(|p| !p.link.is_empty())
            .map(|p| p.buffer)
            .collect();
        assert_eq!(
            attached,
            vec![p2],
            "only one pane may be active per direct link"
        );

        // p1 is detached: writes are refused, buffer survives.
        let resp = st
            .handle(Request::Write {
                buffer: p1,
                data: protocol::to_hex(b"x"),
            })
            .await;
        assert!(!resp.ok);
        assert!(
            st.buffers.lock().await.contains_key(&p1),
            "pane slot must survive"
        );
    }

    #[tokio::test]
    async fn delete_link_closes_the_transport_and_keeps_pane_buffers() {
        let link = LinkId::new();
        let pane = BufferId::new();
        let fake = Fake::new(vec![b"keep-me"]);
        let st = state_with_link(link, Arc::clone(&fake));
        st.handle(Request::CreateBuffer { buffer: pane }).await;
        st.handle(attach(pane, link)).await;
        assert_eq!(read_pane(&st, pane, 32).await, b"keep-me");

        assert!(st.handle(Request::DeleteLink { link }).await.ok);
        assert_eq!(
            fake.closed.load(Ordering::Relaxed),
            1,
            "transport closed once"
        );

        // The pane is orphaned in the daemon, and its history is intact.
        let status = st.status().await;
        assert!(status.links.is_empty());
        assert!(status
            .buffers
            .iter()
            .any(|p| p.buffer == pane && p.link.is_empty()));
        assert_eq!(
            status
                .buffers
                .iter()
                .find(|p| p.buffer == pane)
                .map(|p| p.buffered),
            Some(7)
        );
        assert_eq!(read_pane(&st, pane, 32).await, b"keep-me");
    }

    #[tokio::test]
    async fn delete_pane_drops_the_slot() {
        let pane = BufferId::new();
        let st = DaemonState::new();
        st.handle(Request::CreateBuffer { buffer: pane }).await;
        assert!(st.handle(Request::DeleteBuffer { buffer: pane }).await.ok);
        assert!(!st.buffers.lock().await.contains_key(&pane));
        let resp = st
            .handle(Request::Read {
                buffer: pane,
                max: 16,
            })
            .await;
        assert!(!resp.ok);
    }

    #[tokio::test]
    async fn create_pane_is_idempotent_and_keeps_bytes() {
        let pane = BufferId::new();
        let st = DaemonState::new();
        st.handle(Request::CreateBuffer { buffer: pane }).await;
        st.buffers
            .lock()
            .await
            .get_mut(&pane)
            .unwrap()
            .buffer
            .lock()
            .await
            .push_slice(b"kept");
        st.handle(Request::CreateBuffer { buffer: pane }).await;
        assert_eq!(read_pane(&st, pane, 16).await, b"kept");
    }

    #[tokio::test]
    async fn status_reports_links_panes_and_buffer_sizes() {
        let link = LinkId::new();
        let pane = BufferId::new();
        let st = state_with_link(link, Fake::new(vec![b"12345"]));
        st.handle(Request::CreateBuffer { buffer: pane }).await;
        st.handle(attach(pane, link)).await;
        let _ = read_pane(&st, pane, 16).await;

        let resp = st.handle(Request::Status).await;
        let status = resp.status.expect("status field");
        assert_eq!(status.pid, std::process::id());
        assert_eq!(status.links, vec![link]);
        assert_eq!(status.buffers.len(), 1);
        assert_eq!(status.buffers[0].link, link.to_string());
        assert_eq!(status.buffers[0].buffered, 5);
    }

    #[tokio::test]
    async fn idle_exit_waits_for_links_to_clear() {
        let st = DaemonState::new();
        let link = LinkId::new();
        // No links + stale activity ⇒ exit.
        *st.last_activity.lock().await = tokio::time::Instant::now() - IDLE_TIMEOUT;
        assert!(st.should_exit_idle().await);

        // With a link, never exit — that transport is the whole point.
        st.links.try_lock().unwrap().insert(link, Fake::new(vec![]));
        assert!(!st.should_exit_idle().await);
    }

    #[tokio::test]
    async fn attach_to_an_unknown_link_is_refused() {
        let st = DaemonState::new();
        let pane = BufferId::new();
        let resp = st.handle(attach(pane, LinkId::new())).await;
        assert!(!resp.ok);
        assert!(resp
            .error
            .unwrap_or_default()
            .contains("not held by the daemon"));
    }

    #[tokio::test]
    async fn probe_and_close_an_unknown_tmux_session_are_harmless() {
        let st = DaemonState::new();
        let sid = SessionId::new();
        let probe = st.handle(Request::ProbeTmux { session: sid }).await;
        assert!(probe.ok);
        assert_eq!(probe.alive, Some(false));

        // Closing a slot that was never opened is the state the caller asked
        // for, and must not invent one.
        assert!(st.handle(Request::CloseTmux { session: sid }).await.ok);
        assert!(st.tmux.lock().await.is_empty());

        // A structure query for a session the daemon was never told about is
        // an error naming the reason, not an empty structure.
        let resp = st.handle(Request::Structure { session: sid }).await;
        assert!(!resp.ok);
        assert!(resp.error.unwrap_or_default().contains("open it first"));
    }

    #[tokio::test]
    async fn a_tmux_slot_blocks_idle_exit() {
        // The gate is "no links AND no control slots": a managed session must
        // keep the daemon (and therefore its buffers) alive.
        assert!(idle_ready(false, false, IDLE_TIMEOUT, IDLE_TIMEOUT));
        assert!(
            !idle_ready(true, false, IDLE_TIMEOUT, IDLE_TIMEOUT),
            "a link must hold the daemon"
        );
        assert!(
            !idle_ready(false, true, IDLE_TIMEOUT, IDLE_TIMEOUT),
            "a control slot must hold the daemon"
        );
        assert!(
            !idle_ready(false, false, IDLE_TIMEOUT / 2, IDLE_TIMEOUT),
            "not idle long enough yet"
        );
    }

    #[tokio::test]
    async fn control_pane_writes_need_an_attached_control_client() {
        // A write to a pane that is not in any tmux slot falls through to the
        // native path, which says "not attached" (the message the CLI shows).
        let st = DaemonState::new();
        let pane = BufferId::new();
        st.handle(Request::CreateBuffer { buffer: pane }).await;
        let resp = st
            .handle(Request::Write {
                buffer: pane,
                data: protocol::to_hex(b"x"),
            })
            .await;
        assert!(!resp.ok);
        assert!(resp.error.unwrap_or_default().contains("not attached"));
        // ...and a read of a pane with no slot at all is refused, not empty.
        let resp = st
            .handle(Request::Read {
                buffer: BufferId::new(),
                max: 8,
            })
            .await;
        assert!(!resp.ok);
    }
}
