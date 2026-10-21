// daemon/server.rs (v0.5.0 — resident daemon, ADR 0009)
//
// `xscore daemon serve` — the process that owns what a CLI process
// cannot: live direct-link transports and pane ring buffers.
//
// Why a single daemon for every link (ADR 0009 § 1): a pane may switch
// links at any time, and its buffer has to follow it. Buffers can only
// follow panes inside one process, so there is exactly one daemon and it
// holds both maps:
//
//     links: HashMap<LinkId, Arc<dyn Transport>>
//     panes: HashMap<PaneId, PaneSlot { link, buffer, pump }>
//
// The CLI keeps the authoritative *metadata* (`state.json`: which pane is
// bound to which link). This process only owns *bytes*. That split is why
// `link add` still works with the daemon stopped, and why killing the
// daemon never corrupts the user's model — it just makes direct links
// report `connected: false` until something re-creates them.
//
// Lifecycle: bind (handling a stale socket file), accept, and exit on
//   * `Stop` (explicit `xscore daemon stop`),
//   * 5 s of idleness **with no links** (so tests and one-off sessions
//     never leave a daemon behind; a daemon holding a link stays up,
//     because that link's transport is the whole point).
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use tokio::net::{UnixListener, UnixStream};
use tokio::sync::{Mutex, Notify};
use tokio::task::JoinHandle;

use crate::backends::bridge::DIRECT_BUFFER_CAPACITY;
use crate::backends::RingBuffer;
use crate::core::{Error, LinkId, PaneId, Result};
use crate::transport::{factory as transport_factory, Transport};

use super::client::socket_path;
use super::protocol::{self, PaneStatus, Request, Response, Status};

/// Same polling strategy as `DirectBridge` (v0.4.1): transport `read`
/// blocks until bytes arrive and transports may serialize read/write
/// behind one lock, so we cancel the read every 10 ms to give writers a
/// window. Serial's own read timeout (5 ms) is shorter and stays as is.
const READ_POLL: Duration = Duration::from_millis(10);

/// How long a `Read` waits for the pump to produce bytes before
/// reporting "nothing available" — identical to `DirectBridge::read`.
const READ_WAIT: Duration = Duration::from_millis(200);

/// Idle exit: no links + no requests for this long ⇒ exit.
const IDLE_TIMEOUT: Duration = Duration::from_secs(5);
const IDLE_TICK: Duration = Duration::from_millis(100);

struct PaneSlot {
    /// Currently attached link, if any. Cleared by Detach/DeleteLink —
    /// the buffer survives either way (that is the point of a pane).
    link: Option<LinkId>,
    buffer: Arc<Mutex<RingBuffer>>,
    /// The reader task feeding `buffer` from `link`'s transport.
    pump: Option<JoinHandle<()>>,
}

impl PaneSlot {
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

struct DaemonState {
    links: Mutex<HashMap<LinkId, Arc<dyn Transport>>>,
    panes: Mutex<HashMap<PaneId, PaneSlot>>,
    last_activity: Mutex<tokio::time::Instant>,
}

impl DaemonState {
    fn new() -> Self {
        Self {
            links: Mutex::new(HashMap::new()),
            panes: Mutex::new(HashMap::new()),
            last_activity: Mutex::new(tokio::time::Instant::now()),
        }
    }

    async fn touch(&self) {
        *self.last_activity.lock().await = tokio::time::Instant::now();
    }

    /// Exit when nothing is left to serve. A daemon with a link must stay
    /// up — that transport is the entire reason it exists.
    async fn should_exit_idle(&self) -> bool {
        if !self.links.lock().await.is_empty() {
            return false;
        }
        self.last_activity.lock().await.elapsed() >= IDLE_TIMEOUT
    }

    /// Abort every pump and close every transport (daemon shutdown).
    async fn shutdown(&self) {
        {
            let mut panes = self.panes.lock().await;
            for slot in panes.values_mut() {
                slot.stop_pump();
            }
        }
        let links: Vec<Arc<dyn Transport>> =
            self.links.lock().await.drain().map(|(_, t)| t).collect();
        for t in links {
            let _ = t.close().await;
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
                    let mut panes = self.panes.lock().await;
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

            Request::CreatePane { pane } => {
                self.panes.lock().await.entry(pane).or_insert_with(PaneSlot::new);
                Ok(Response::ok())
            }

            Request::DeletePane { pane } => {
                if let Some(mut slot) = self.panes.lock().await.remove(&pane) {
                    slot.stop_pump();
                }
                Ok(Response::ok())
            }

            Request::Attach { pane, link } => {
                // Never hold both locks: `links` is cloned out first so
                // the ordering (links → panes) can't invert anywhere.
                let transport = self
                    .links
                    .lock()
                    .await
                    .get(&link)
                    .cloned()
                    .ok_or_else(|| format!("link {link} is not held by the daemon"))?;

                let mut panes = self.panes.lock().await;
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
                let slot = panes.entry(pane).or_insert_with(PaneSlot::new);
                if slot.link == Some(link) && slot.pump.is_some() {
                    return Ok(Response::ok());
                }
                slot.stop_pump();
                slot.link = Some(link);
                slot.pump = Some(spawn_pump(transport, Arc::clone(&slot.buffer)));
                Ok(Response::ok())
            }

            Request::Detach { pane } => {
                if let Some(slot) = self.panes.lock().await.get_mut(&pane) {
                    slot.stop_pump();
                    slot.link = None;
                }
                // Idempotent: detaching an unknown/already-detached pane
                // is the state the caller asked for.
                Ok(Response::ok())
            }

            Request::Read { pane, max } => {
                let buffer = {
                    let panes = self.panes.lock().await;
                    Arc::clone(
                        &panes
                            .get(&pane)
                            .ok_or_else(|| format!("pane {pane} has no buffer in the daemon"))?
                            .buffer,
                    )
                };
                let max = max.max(1);
                let deadline = tokio::time::Instant::now() + READ_WAIT;
                loop {
                    {
                        let ring = buffer.lock().await;
                        if !ring.is_empty() {
                            let mut out = vec![0u8; max.min(ring.len())];
                            let n = ring.copy_out(&mut out);
                            out.truncate(n);
                            return Ok(Response::bytes(&out));
                        }
                    }
                    if tokio::time::Instant::now() >= deadline {
                        return Ok(Response::bytes(&[]));
                    }
                    tokio::time::sleep(READ_POLL).await;
                }
            }

            Request::Write { pane, data } => {
                let bytes = protocol::from_hex(&data).map_err(|e| e.to_string())?;
                let transport = self.pane_transport(pane).await?;
                let n = transport.write(&bytes).await.map_err(|e| e.to_string())?;
                Ok(Response::count(n))
            }

            Request::Resize { pane, cols, rows } => {
                let transport = self.pane_transport(pane).await?;
                transport
                    .resize(cols, rows)
                    .await
                    .map_err(|e| e.to_string())?;
                Ok(Response::ok())
            }

            Request::Stop => Ok(Response::ok()),
        }
    }

    /// The transport behind a pane's current link. The error text mirrors
    /// `core::Error::NotAttached` so the CLI's message stays the same as
    /// it was when the bridge lived in-process.
    async fn pane_transport(&self, pane: PaneId) -> std::result::Result<Arc<dyn Transport>, String> {
        let link = {
            let panes = self.panes.lock().await;
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

        let mut panes = Vec::new();
        for (pane, slot) in self.panes.lock().await.iter() {
            panes.push(PaneStatus {
                pane: *pane,
                link: slot.link.map(|l| l.to_string()).unwrap_or_default(),
                buffered: slot.buffer.lock().await.len(),
            });
        }
        panes.sort_by_key(|p| p.pane.to_string());

        Status { pid: std::process::id(), links, panes }
    }
}

/// Build + open a transport for a link. `open()` is normally called by
/// `sink_for(RawSink)` in the CLI; the daemon has no sink, so it opens
/// here — without it `ShellTransport::read/write` fail with "not open".
async fn create_transport(config: crate::model::TransportConfig) -> std::result::Result<Arc<dyn Transport>, String> {
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
pub async fn run() -> Result<()> {
    // Ignore SIGHUP: the daemon may have been started by a `link add`
    // that ran in a terminal which is since closed. Dropping every live
    // direct link because a terminal went away is never what the user
    // wants — links are only torn down explicitly (`link del`) or when
    // the daemon is idle. SIGTERM/SIGINT are left alone so `daemon stop`
    // and a deliberate Ctrl-C on `daemon serve` still work.
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
        std::fs::create_dir_all(dir).map_err(|e| {
            Error::Daemon(format!("mkdir {}: {e}", dir.display()))
        })?;
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
                eprintln!(
                    "xscore daemon: already running at {}",
                    path.display()
                );
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
            _ = tokio::time::sleep(IDLE_TICK) => {
                if state.should_exit_idle().await {
                    tracing::info!("daemon exiting: idle with no links");
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
                    if UnixStream::connect(socket).await.is_ok() {
                        return Ok(false);
                    }
                    tokio::time::sleep(Duration::from_millis(20)).await;
                }
                // Stale: take it over. If another process beats us to it,
                // its `create_new` wins and ours reports AlreadyExists on
                // the next pass, so at most one daemon ends up serving.
                let _ = std::fs::remove_file(lock_path);
            }
            Err(e) => {
                return Err(Error::Daemon(format!(
                    "lock {}: {e}",
                    lock_path.display()
                )))
            }
        }
    }
    Ok(false)
}

/// One connection: read requests until EOF. The CLI opens one connection
/// per request, but looping keeps a long-lived client (a future TUI) from
/// needing protocol changes.
async fn serve_conn(mut stream: UnixStream, state: Arc<DaemonState>, shutdown: Arc<Notify>) {
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
    use async_trait::async_trait;
    use crate::core::LinkAddress;
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
        fn kind(&self) -> &'static str { "fake" }
        fn address(&self) -> LinkAddress {
            LinkAddress::Shell { command: String::new(), args: vec![] }
        }
        async fn open(&mut self) -> Result<()> { Ok(()) }
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
        async fn resize(&self, _: u16, _: u16) -> Result<()> { Ok(()) }
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

    async fn read_pane(st: &DaemonState, pane: PaneId, max: usize) -> Vec<u8> {
        let resp = st.handle(Request::Read { pane, max }).await;
        assert!(resp.ok, "read failed: {:?}", resp.error);
        protocol::from_hex(resp.data.as_deref().unwrap_or("")).unwrap()
    }

    #[tokio::test]
    async fn attach_pumps_bytes_into_the_pane_buffer() {
        let link = LinkId::new();
        let pane = PaneId::new();
        let fake = Fake::new(vec![b"hello\n"]);
        let st = state_with_link(link, Arc::clone(&fake));

        st.handle(Request::CreatePane { pane }).await;
        assert!(st.handle(Request::Attach { pane, link }).await.ok);

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
        let pane = PaneId::new();
        let fake = Fake::new(vec![]);
        let st = state_with_link(link, Arc::clone(&fake));
        st.handle(Request::CreatePane { pane }).await;
        st.handle(Request::Attach { pane, link }).await;

        let resp = st
            .handle(Request::Write { pane, data: protocol::to_hex(b"abc") })
            .await;
        assert!(resp.ok, "{:?}", resp.error);
        assert_eq!(resp.n, Some(3));
        assert_eq!(&*fake.written.lock().await, b"abc");
    }

    #[tokio::test]
    async fn write_without_an_attached_link_is_refused() {
        let pane = PaneId::new();
        let st = DaemonState::new();
        st.handle(Request::CreatePane { pane }).await;
        let resp = st
            .handle(Request::Write { pane, data: protocol::to_hex(b"x") })
            .await;
        assert!(!resp.ok);
        let err = resp.error.unwrap_or_default();
        // Same wording the in-process bridge produced (NotAttached).
        assert!(err.contains("not attached"), "got: {err}");
    }

    #[tokio::test]
    async fn detach_keeps_the_buffer_and_stops_the_pump() {
        let link = LinkId::new();
        let pane = PaneId::new();
        let fake = Fake::new(vec![b"abc"]);
        let st = state_with_link(link, fake);
        st.handle(Request::CreatePane { pane }).await;
        st.handle(Request::Attach { pane, link }).await;
        assert_eq!(read_pane(&st, pane, 16).await, b"abc");

        assert!(st.handle(Request::Detach { pane }).await.ok);
        // Buffer survives the detach ...
        assert_eq!(read_pane(&st, pane, 16).await, b"abc");
        // ... but the pane is no longer writable.
        let resp = st
            .handle(Request::Write { pane, data: protocol::to_hex(b"z") })
            .await;
        assert!(!resp.ok);
    }

    #[tokio::test]
    async fn attach_auto_detaches_the_previous_pane_q1() {
        let link = LinkId::new();
        let p1 = PaneId::new();
        let p2 = PaneId::new();
        let fake = Fake::new(vec![]);
        let st = state_with_link(link, fake);
        for p in [p1, p2] {
            st.handle(Request::CreatePane { pane: p }).await;
        }
        st.handle(Request::Attach { pane: p1, link }).await;
        st.handle(Request::Attach { pane: p2, link }).await;

        let status = st.status().await;
        let attached: Vec<PaneId> = status
            .panes
            .iter()
            .filter(|p| !p.link.is_empty())
            .map(|p| p.pane)
            .collect();
        assert_eq!(attached, vec![p2], "only one pane may be active per direct link");

        // p1 is detached: writes are refused, buffer survives.
        let resp = st.handle(Request::Write { pane: p1, data: protocol::to_hex(b"x") }).await;
        assert!(!resp.ok);
        assert!(st.panes.lock().await.contains_key(&p1), "pane slot must survive");
    }

    #[tokio::test]
    async fn delete_link_closes_the_transport_and_keeps_pane_buffers() {
        let link = LinkId::new();
        let pane = PaneId::new();
        let fake = Fake::new(vec![b"keep-me"]);
        let st = state_with_link(link, Arc::clone(&fake));
        st.handle(Request::CreatePane { pane }).await;
        st.handle(Request::Attach { pane, link }).await;
        assert_eq!(read_pane(&st, pane, 32).await, b"keep-me");

        assert!(st.handle(Request::DeleteLink { link }).await.ok);
        assert_eq!(fake.closed.load(Ordering::Relaxed), 1, "transport closed once");

        // The pane is orphaned in the daemon, and its history is intact.
        let status = st.status().await;
        assert!(status.links.is_empty());
        assert!(status.panes.iter().any(|p| p.pane == pane && p.link.is_empty()));
        assert_eq!(
            status.panes.iter().find(|p| p.pane == pane).map(|p| p.buffered),
            Some(7)
        );
        assert_eq!(read_pane(&st, pane, 32).await, b"keep-me");
    }

    #[tokio::test]
    async fn delete_pane_drops_the_slot() {
        let pane = PaneId::new();
        let st = DaemonState::new();
        st.handle(Request::CreatePane { pane }).await;
        assert!(st.handle(Request::DeletePane { pane }).await.ok);
        assert!(!st.panes.lock().await.contains_key(&pane));
        let resp = st.handle(Request::Read { pane, max: 16 }).await;
        assert!(!resp.ok);
    }

    #[tokio::test]
    async fn create_pane_is_idempotent_and_keeps_bytes() {
        let pane = PaneId::new();
        let st = DaemonState::new();
        st.handle(Request::CreatePane { pane }).await;
        st.panes
            .lock()
            .await
            .get_mut(&pane)
            .unwrap()
            .buffer
            .lock()
            .await
            .push_slice(b"kept");
        st.handle(Request::CreatePane { pane }).await;
        assert_eq!(read_pane(&st, pane, 16).await, b"kept");
    }

    #[tokio::test]
    async fn status_reports_links_panes_and_buffer_sizes() {
        let link = LinkId::new();
        let pane = PaneId::new();
        let st = state_with_link(link, Fake::new(vec![b"12345"]));
        st.handle(Request::CreatePane { pane }).await;
        st.handle(Request::Attach { pane, link }).await;
        let _ = read_pane(&st, pane, 16).await;

        let resp = st.handle(Request::Status).await;
        let status = resp.status.expect("status field");
        assert_eq!(status.pid, std::process::id());
        assert_eq!(status.links, vec![link]);
        assert_eq!(status.panes.len(), 1);
        assert_eq!(status.panes[0].link, link.to_string());
        assert_eq!(status.panes[0].buffered, 5);
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
        let pane = PaneId::new();
        let resp = st.handle(Request::Attach { pane, link: LinkId::new() }).await;
        assert!(!resp.ok);
        assert!(resp.error.unwrap_or_default().contains("not held by the daemon"));
    }
}
