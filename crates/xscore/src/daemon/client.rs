// daemon/client.rs (v0.5.0 — resident daemon, ADR 0009)
//
// The CLI's side of the daemon. Three things live here:
//
//   1. `socket_path()` — where the socket is.
//   2. `ensure_daemon()` / `send()` / `probe*()` — the auto-starting,
//      one-connection-per-request transport.
//   3. `DaemonClient` + `DaemonTransport` — the `PaneBridge`- and
//      `Transport`-shaped adapters the rest of the crate already knows
//      how to hold, so nothing else had to change shape.
//
// One connection per request (rather than a pool) is deliberate: the CLI
// is a short-lived process doing a handful of ops, and a stateless
// request keeps `DaemonClient` `Send + Sync + Copy`-ish with no broken
// pipe to re-establish after a daemon restart.
use std::path::PathBuf;
use std::time::Duration;

use async_trait::async_trait;

use crate::core::{Error, LinkAddress, LinkId, PaneId, Result, SessionId};
use crate::model::{TmuxHostConfig, TmuxSocket, TransportConfig};
use crate::transport::Transport;

use super::ipc;
use super::protocol::{self, Request, Response, Status, TmuxStructure};

/// How long `ensure_daemon` waits for a spawned daemon to accept.
const START_TIMEOUT: Duration = Duration::from_secs(2);
const START_POLL: Duration = Duration::from_millis(20);

/// `$XDG_RUNTIME_DIR/xscore/daemon.sock`, else
/// `$HOME/.config/xscore/run/daemon.sock`. Never a hard-coded `/tmp`
/// path: two users (or two test HOMEs) must never share a socket.
pub fn socket_path() -> PathBuf {
    socket_path_from(std::env::var_os("XDG_RUNTIME_DIR").as_deref())
}

/// Pure core of `socket_path` — split out so it is testable without
/// mutating the process environment (other tests read `HOME`).
pub fn socket_path_from(xdg_runtime_dir: Option<&std::ffi::OsStr>) -> PathBuf {
    if let Some(rt) = xdg_runtime_dir {
        if !rt.is_empty() {
            return PathBuf::from(rt).join("xscore").join("daemon.sock");
        }
    }
    crate::config::paths::config_dir().join("run").join("daemon.sock")
}

/// Is something accepting on our local endpoint right now? Connect-only: this is
/// used by `ensure_daemon`'s poll loop and must never *start* a daemon.
pub async fn probe() -> bool {
    ipc::probe(&socket_path()).await
}

/// Send one request to a **running** daemon. IO failures (including "no
/// daemon") come back as `Error::Daemon`; a protocol-level `ok:false` is
/// returned in the `Response` so callers that want the raw answer
/// (`daemon status`) can have it.
pub async fn send(req: &Request) -> Result<Response> {
    send_to(&socket_path(), req).await
}

/// `send` against an explicit path — used by tests (and by nothing else)
/// so "no daemon there" can be exercised without touching the env.
pub async fn send_to(path: &std::path::Path, req: &Request) -> Result<Response> {
    let mut stream = ipc::connect(path).await.map_err(|e| {
        Error::Daemon(format!(
            "no daemon at {} ({e})",
            ipc::display_address(path)
        ))
    })?;
    protocol::write_frame(&mut *stream, req)
        .await
        .map_err(|e| Error::Daemon(format!("write request: {e}")))?;
    let resp: Response = protocol::read_frame(&mut *stream)
        .await
        .map_err(|e| Error::Daemon(format!("read response: {e}")))?
        .ok_or_else(|| Error::Daemon("daemon closed the connection".into()))?;
    Ok(resp)
}

/// Start the daemon if it isn't already up, then wait (bounded) until it
/// accepts. Auto-start is what makes the daemon invisible to the user:
/// the first `link add` after a reboot brings it up.
pub async fn ensure_daemon() -> Result<()> {
    if probe().await {
        return Ok(());
    }
    spawn_serve()?;
    let deadline = tokio::time::Instant::now() + START_TIMEOUT;
    while tokio::time::Instant::now() < deadline {
        if probe().await {
            return Ok(());
        }
        tokio::time::sleep(START_POLL).await;
    }
    Err(Error::Daemon(format!(
        "daemon did not start within {}ms ({})",
        START_TIMEOUT.as_millis(),
        ipc::display_address(&socket_path())
    )))
}

/// Detached spawn of `xscore daemon serve`. The child's stdio is closed
/// so it can never hold the caller's terminal open, and we deliberately
/// do not `wait()` — the daemon must outlive this process.
///
/// The daemon is placed in its OWN SESSION via `setsid` when available.
/// Without that it inherits the caller's session and process group,
/// which means:
///
///   * closing the terminal that happened to run `link add` sends SIGHUP
///     and drops every live direct link,
///   * Ctrl-C at that terminal sends SIGINT to the daemon too,
///   * any wrapper that waits for the caller's whole process group (test
///     harnesses, editors, CI runners) blocks until the daemon exits —
///     which it never does while a link is alive.
///
/// `setsid` ships with util-linux on every Linux distro we target. If it
/// is somehow missing we fall back to a plain spawn: still functional,
/// just not session-isolated.
#[cfg(unix)]
fn spawn_serve() -> Result<()> {
    let exe = std::env::current_exe()
        .map_err(|e| Error::Daemon(format!("current_exe: {e}")))?;
    let spawn = |cmd: &mut std::process::Command| {
        cmd.args(["daemon", "serve", "--idle-exit"])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
    };

    let mut detached = std::process::Command::new("setsid");
    detached.arg(&exe);
    match spawn(&mut detached) {
        Ok(_) => return Ok(()),
        // No `setsid` on PATH — fall back to spawning the binary directly.
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(Error::Daemon(format!("spawn daemon: {e}"))),
    }

    let mut plain = std::process::Command::new(&exe);
    spawn(&mut plain).map_err(|e| Error::Daemon(format!("spawn daemon: {e}")))?;
    Ok(())
}

#[cfg(windows)]
fn spawn_serve() -> Result<()> {
    use std::os::windows::process::CommandExt;

    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    const DETACHED_PROCESS: u32 = 0x0000_0008;

    let exe =
        std::env::current_exe().map_err(|e| Error::Daemon(format!("current_exe: {e}")))?;
    std::process::Command::new(exe)
        .args(["daemon", "serve", "--idle-exit"])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .creation_flags(CREATE_NO_WINDOW | DETACHED_PROCESS)
        .spawn()
        .map_err(|e| Error::Daemon(format!("spawn daemon: {e}")))?;
    Ok(())
}

/// Does the daemon currently hold this link's live transport? This is the
/// answer `link show` prints as `connected` for direct links.
pub async fn probe_link(link: LinkId) -> Result<bool> {
    Ok(fetch_status().await?.links.contains(&link))
}

/// Does the daemon hold a buffer slot for this pane (even detached)?
pub async fn probe_pane(pane: PaneId) -> Result<bool> {
    Ok(fetch_status().await?.panes.iter().any(|p| p.pane == pane))
}

/// Is this session's tmux control client alive inside the daemon? This is what
/// a `Control` link's `connected` reports (ADR 0010 §4: the link IS the
/// client, so "is the link up" and "is the session streaming" are one
/// question).
///
/// A daemon that is not running answers `Err`, which the caller must read as
/// "no answer", not as "disconnected": a stopped daemon holds no buffers but
/// says nothing about the tmux session itself.
pub async fn probe_tmux(session: SessionId) -> Result<bool> {
    let resp = send(&Request::ProbeTmux { session }).await?;
    if !resp.ok {
        return Err(Error::Daemon(
            resp.error.unwrap_or_else(|| "probe failed".into()),
        ));
    }
    Ok(resp.alive.unwrap_or(false))
}

fn structure_of(resp: Response) -> Result<TmuxStructure> {
    resp.structure
        .ok_or_else(|| Error::Daemon("response has no `structure` field".into()))
}

pub async fn fetch_status() -> Result<Status> {
    let resp = send(&Request::Status).await?;
    if !resp.ok {
        return Err(Error::Daemon(
            resp.error.unwrap_or_else(|| "status failed".into()),
        ));
    }
    resp.status
        .ok_or_else(|| Error::Daemon("status response has no `status` field".into()))
}

/// A stateless handle to the daemon. Cheap to clone (it is empty); the
/// manager holds one `Arc<DaemonClient>` for the whole process.
#[derive(Debug, Default, Clone, Copy)]
pub struct DaemonClient;

impl DaemonClient {
    pub fn new() -> Self {
        Self
    }

    /// Make sure a daemon is up (auto-start if needed).
    pub async fn ensure(&self) -> Result<()> {
        ensure_daemon().await
    }

    /// Send + require `ok:true`, turning a daemon error into `Err`.
    pub async fn call(&self, req: Request) -> Result<Response> {
        let resp = send(&req).await?;
        if resp.ok {
            Ok(resp)
        } else {
            Err(Error::Daemon(
                resp.error.unwrap_or_else(|| "daemon reported failure".into()),
            ))
        }
    }

    pub async fn create_link(&self, link: LinkId, transport: TransportConfig) -> Result<()> {
        self.call(Request::CreateLink { link, transport }).await.map(|_| ())
    }

    pub async fn delete_link(&self, link: LinkId) -> Result<()> {
        self.call(Request::DeleteLink { link }).await.map(|_| ())
    }

    pub async fn create_pane(&self, pane: PaneId) -> Result<()> {
        self.call(Request::CreatePane { pane }).await.map(|_| ())
    }

    pub async fn delete_pane(&self, pane: PaneId) -> Result<()> {
        self.call(Request::DeletePane { pane }).await.map(|_| ())
    }

    pub async fn attach(&self, pane: PaneId, link: LinkId) -> Result<()> {
        self.call(Request::Attach { pane, link: Some(link), key: None, session: None })
            .await
            .map(|_| ())
    }

    /// Subscribe a tmux pane's `%output %N` into the daemon's buffer for it.
    /// Idempotent: a pane that is already attached is left alone.
    pub async fn attach_pane(&self, session: SessionId, pane: PaneId, key: &str) -> Result<()> {
        self.call(Request::Attach {
            pane,
            link: None,
            key: Some(key.to_string()),
            session: Some(session),
        })
        .await
        .map(|_| ())
    }

    /// Open (or reuse) the session's control client and return its structure.
    /// Auto-starts the daemon: a managed session is exactly what the daemon
    /// exists to hold.
    pub async fn open_tmux(
        &self,
        session: SessionId,
        host: TmuxHostConfig,
        socket: TmuxSocket,
        name: &str,
        cols: u16,
        rows: u16,
    ) -> Result<TmuxStructure> {
        self.ensure().await?;
        let resp = self
            .call(Request::OpenTmux {
                session,
                host,
                socket,
                name: name.to_string(),
                cols,
                rows,
            })
            .await?;
        structure_of(resp)
    }

    /// Enumerate the session through the client the daemon already holds.
    pub async fn structure(&self, session: SessionId) -> Result<TmuxStructure> {
        structure_of(self.call(Request::Structure { session }).await?)
    }

    /// Run one command on the session's control client (window/pane
    /// mutations: `new-window`, `split-window`, `kill-pane`).
    pub async fn tmux_command(&self, session: SessionId, command: &str) -> Result<Vec<String>> {
        let resp = self
            .call(Request::TmuxCommand { session, command: command.to_string() })
            .await?;
        Ok(resp.lines.unwrap_or_default())
    }

    /// Close the client and drop its buffers (kill/forget).
    pub async fn close_tmux(&self, session: SessionId) -> Result<()> {
        self.call(Request::CloseTmux { session }).await.map(|_| ())
    }

    /// Is the server session's client alive in the daemon? `false` when the
    /// daemon is not running at all.
    pub async fn probe_tmux(&self, session: SessionId) -> bool {
        probe_tmux(session).await.unwrap_or(false)
    }

    pub async fn detach(&self, pane: PaneId) -> Result<()> {
        self.call(Request::Detach { pane }).await.map(|_| ())
    }

    pub async fn read(&self, pane: PaneId, max: usize) -> Result<Vec<u8>> {
        let resp = self.call(Request::Read { pane, max }).await?;
        protocol::from_hex(resp.data.as_deref().unwrap_or(""))
    }

    pub async fn write(&self, pane: PaneId, data: &[u8]) -> Result<usize> {
        let resp = self
            .call(Request::Write { pane, data: protocol::to_hex(data) })
            .await?;
        // The daemon knows how many bytes the transport accepted; fall
        // back to the input length if an older daemon omits it.
        Ok(resp.n.unwrap_or(data.len()))
    }

    pub async fn resize(&self, pane: PaneId, cols: u16, rows: u16) -> Result<()> {
        self.call(Request::Resize { pane, cols, rows }).await.map(|_| ())
    }

    pub async fn status(&self) -> Result<Status> {
        fetch_status().await
    }
}

/// The CLI-side `Transport` for a daemon-owned direct link.
///
/// Nothing performs I/O through it: pane reads/writes go through
/// `DaemonBridge` (they need a pane id, and the daemon — not the CLI —
/// pumps bytes into the pane's buffer). The link object still needs a
/// `Transport` because `Link::transport` is part of the model, so this
/// adapter is that placeholder: `open`/`close` are no-ops (the daemon owns
/// the real transport's lifetime, and closes it on `delete_link`), and
/// direct I/O is refused loudly rather than silently doing nothing.
pub struct DaemonTransport {
    link: LinkId,
}

impl std::fmt::Debug for DaemonTransport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DaemonTransport")
            .field("link", &self.link)
            .finish_non_exhaustive()
    }
}

impl DaemonTransport {
    pub fn new(link: LinkId) -> Self {
        Self { link }
    }
}

#[async_trait]
impl Transport for DaemonTransport {
    fn kind(&self) -> &'static str {
        "daemon"
    }

    fn address(&self) -> LinkAddress {
        // The routing address lives in `LinkInfo.address` (CLI-side
        // metadata); this adapter has no opinion about it.
        LinkAddress::Shell { command: String::new(), args: vec![] }
    }

    async fn open(&mut self) -> Result<()> {
        Ok(())
    }

    async fn write(&self, _data: &[u8]) -> Result<usize> {
        Err(Error::Daemon(format!(
            "link {} is daemon-owned; write through the pane's bridge, not the link transport",
            self.link
        )))
    }

    async fn read(&self, _buf: &mut [u8]) -> Result<usize> {
        Err(Error::Daemon(format!(
            "link {} is daemon-owned; read the pane's buffer, not the link transport",
            self.link
        )))
    }

    async fn resize(&self, _cols: u16, _rows: u16) -> Result<()> {
        Err(Error::Daemon(format!(
            "link {} is daemon-owned; resize the pane's bridge, not the link transport",
            self.link
        )))
    }

    /// No-op on purpose: `delete_link` is the single teardown path, and
    /// the manager calls it explicitly. Closing here would be a second,
    /// racing way to kill the same transport.
    async fn close(&self) -> Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::daemon::protocol::PaneStatus;

    #[test]
    fn socket_path_is_under_xdg_runtime_dir_when_set() {
        let p = socket_path_from(Some(std::ffi::OsStr::new("/run/user/1234")));
        assert_eq!(p, PathBuf::from("/run/user/1234/xscore/daemon.sock"));
    }

    #[test]
    fn socket_path_falls_back_to_home_config() {
        for xdg in [None, Some(std::ffi::OsStr::new(""))] {
            let p = socket_path_from(xdg);
            assert!(
                p.ends_with(".config/xscore/run/daemon.sock"),
                "unexpected fallback path: {}",
                p.display()
            );
        }
    }

    #[tokio::test]
    async fn send_without_a_daemon_is_a_daemon_error_not_a_panic() {
        let missing = std::path::Path::new("/nonexistent-xscore-test/daemon.sock");
        let err = send_to(missing, &Request::Ping)
            .await
            .expect_err("no daemon must be an error");
        assert!(matches!(err, Error::Daemon(_)), "got: {err}");
        assert!(format!("{err}").contains("no daemon"), "got: {err}");
    }

    #[tokio::test]
    async fn daemon_transport_refuses_direct_io() {
        let mut t = DaemonTransport::new(LinkId::new());
        assert_eq!(t.kind(), "daemon");
        t.open().await.unwrap();
        assert!(matches!(t.write(b"x").await, Err(Error::Daemon(_))));
        assert!(matches!(t.read(&mut [0u8; 4]).await, Err(Error::Daemon(_))));
        assert!(matches!(t.resize(80, 24).await, Err(Error::Daemon(_))));
        // close() is a deliberate no-op: delete_link owns teardown.
        t.close().await.unwrap();
    }

    #[test]
    fn pane_status_link_is_empty_string_when_detached() {
        let json = serde_json::to_value(PaneStatus {
            pane: PaneId::parse("01928e0a-7d10-7000-8000-000000000001").unwrap(),
            link: String::new(),
            buffered: 0,
        })
        .unwrap();
        assert_eq!(json["link"], serde_json::json!(""));
    }
}
