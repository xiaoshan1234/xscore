// backends/bridge.rs (v0.5.0 — pane buffers + daemon bridge, ADR 0008/0009)
//
// The bridge is the per-pane runtime that exposes `read` / `write` /
// `resize` / `close`. Three concrete kinds:
//
//   - `DirectBridge`: the pane's BUFFER, in-process. A background task
//     continuously drains the link's `Arc<dyn Transport>` into a
//     `RingBuffer`; `read` serves a snapshot of that buffer. Used by
//     in-process callers (the data_smoke tests, and any embedding) where
//     the transport and the pane live in the same process.
//
//   - `DaemonBridge` (v0.5.0): the SAME design, but the pump and the
//     buffer live in the resident daemon (ADR 0009). Every method is a
//     request over the Unix socket, so a pane's history and its link's
//     transport survive the CLI process that created them.
//
//   - `TmuxBridge`: wraps the link's `TmuxSink` (via `Weak<TmuxSink>` to
//     avoid reference cycles). `read` = `tmux capture-pane` (tmux's own
//     scrollback IS the buffer), `write` = `send-keys`, `resize` =
//     `resize-pane`, `close` = `kill-pane` (a real tmux pane dies).
//     tmux is already resident, so this path never involves the daemon.
//
// `close()` means "release THIS pane's hold on the resource", never
// "kill a shared resource" (P0-a):
//   - DirectBridge::close(): stop the reader task. The link's transport
//     stays OPEN — other panes (and a later re-attach) still use it.
//   - TmuxBridge::close(): kill the tmux pane (it is this pane's alone).
//
// The manager holds the bridges (and, for direct panes, the buffers) in
// maps keyed by PaneId, so detaching a direct pane keeps its history.
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use tokio::sync::Mutex;

use crate::core::Result;
use crate::sink::Sink;
use crate::transport::Transport;

use super::ring_buffer::RingBuffer;

/// Default capacity of a direct pane's buffer (64 KiB of scrollback).
pub const DIRECT_BUFFER_CAPACITY: usize = 64 * 1024;

/// How long `DirectBridge::read` waits for the reader task to produce
/// bytes before reporting "nothing available" (`Ok(0)`).
const READ_WAIT: Duration = Duration::from_millis(200);
const READ_POLL: Duration = Duration::from_millis(10);

/// Per-pane runtime bridge. Object-safe (`dyn PaneBridge` is valid)
/// because every method takes `&self` and the trait has no associated
/// types. `Send + Sync` is required so the bridge can be shared into
/// background tasks and into the data CLI.
#[async_trait]
pub trait PaneBridge: Send + Sync + std::fmt::Debug {
    /// What kind of bridge this is — `"direct"` or `"tmux"`.
    fn kind(&self) -> &'static str;

    /// Read a snapshot of the pane's buffer into `buf`. Returns the
    /// number of bytes copied; `Ok(0)` means "no data available right
    /// now" (after a short wait). Non-destructive: a pane's buffer is
    /// scrollback, not a consumable stream.
    async fn read(&self, buf: &mut [u8]) -> Result<usize>;

    /// Push bytes into the pane. Returns the number of bytes accepted.
    async fn write(&self, data: &[u8]) -> Result<usize>;

    /// Resize the pane's terminal dimensions.
    async fn resize(&self, cols: u16, rows: u16) -> Result<()>;

    /// Release this pane's hold on its I/O resource. Never kills a
    /// resource shared with the link (see the module comment).
    async fn close(&self) -> Result<()>;
}

/// Direct bridge over the link's `Arc<dyn Transport>`, with the pane's
/// own ring buffer fed by a background reader task.
pub struct DirectBridge {
    transport: Arc<dyn Transport>,
    buffer: Arc<Mutex<RingBuffer>>,
    stop: Arc<AtomicBool>,
    task: Arc<Mutex<Option<tokio::task::JoinHandle<()>>>>,
}

impl std::fmt::Debug for DirectBridge {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DirectBridge")
            .field("transport", &self.transport.kind())
            .field("stop", &self.stop.load(Ordering::Relaxed))
            .finish_non_exhaustive()
    }
}

impl DirectBridge {
    /// Create a bridge with a fresh private buffer.
    pub fn new(transport: Arc<dyn Transport>) -> Self {
        Self::new_with_buffer(
            transport,
            Arc::new(Mutex::new(RingBuffer::new(DIRECT_BUFFER_CAPACITY))),
        )
    }

    /// Create a bridge that writes into a caller-owned buffer. The
    /// manager uses this so a detached pane keeps its history and a
    /// re-attach continues appending to the same scrollback.
    pub fn new_with_buffer(
        transport: Arc<dyn Transport>,
        buffer: Arc<Mutex<RingBuffer>>,
    ) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let task = {
            let t = Arc::clone(&transport);
            let b = Arc::clone(&buffer);
            let s = Arc::clone(&stop);
            tokio::spawn(async move {
                let mut buf = [0u8; 4096];
                while !s.load(Ordering::Relaxed) {
                    // `read` blocks until bytes arrive, so a bare `await` would
                    // never re-check `stop` and a detach could not end the
                    // pump. The timeout re-arms the read every READ_POLL and is
                    // the only reason it exists: since ADR 0010 a shell link is
                    // a pty with independent read/write halves, so the old
                    // "release the transport lock" reason is gone, and a
                    // cancelled read loses nothing (the bytes wait in the
                    // transport's channel).
                    let read = tokio::time::timeout(READ_POLL, t.read(&mut buf)).await;
                    match read {
                        Ok(Ok(0)) => tokio::time::sleep(READ_POLL).await,
                        Ok(Ok(n)) => {
                            let mut ring = b.lock().await;
                            ring.push_slice(&buf[..n]);
                        }
                        Ok(Err(_)) => break,
                        // Timed out: no bytes yet. Loop and re-arm.
                        Err(_elapsed) => {}
                    }
                }
            })
        };
        Self {
            transport,
            buffer,
            stop,
            task: Arc::new(Mutex::new(Some(task))),
        }
    }

    /// The buffer this bridge appends to (shared with the manager).
    pub fn buffer(&self) -> Arc<Mutex<RingBuffer>> {
        Arc::clone(&self.buffer)
    }
}

#[async_trait]
impl PaneBridge for DirectBridge {
    fn kind(&self) -> &'static str { "direct" }

    async fn read(&self, buf: &mut [u8]) -> Result<usize> {
        let deadline = tokio::time::Instant::now() + READ_WAIT;
        loop {
            {
                let ring = self.buffer.lock().await;
                let n = ring.copy_out(buf);
                if n > 0 {
                    return Ok(n);
                }
            }
            if tokio::time::Instant::now() >= deadline {
                return Ok(0);
            }
            tokio::time::sleep(READ_POLL).await;
        }
    }

    async fn write(&self, data: &[u8]) -> Result<usize> {
        self.transport.write(data).await
    }

    async fn resize(&self, cols: u16, rows: u16) -> Result<()> {
        self.transport.resize(cols, rows).await
    }

    async fn close(&self) -> Result<()> {
        // P0-a: stop OUR pump only. The link's transport is shared —
        // closing it here killed every other pane on the link and made
        // a later re-attach dead on arrival.
        self.stop.store(true, Ordering::Relaxed);
        let mut slot = self.task.lock().await;
        if let Some(handle) = slot.take() {
            handle.abort();
        }
        Ok(())
    }
}

/// Bridge over the resident daemon (v0.5.0, ADR 0009).
///
/// Same contract as `DirectBridge` from the outside — `read` is a
/// non-destructive snapshot of the pane's scrollback, `write` pushes
/// bytes at the link — but both the buffer and the transport belong to
/// the daemon, so they outlive this process. `close()` releases only
/// this pane's binding (`Detach`); the link's transport keeps running.
#[derive(Debug)]
pub struct DaemonBridge {
    client: Arc<crate::daemon::client::DaemonClient>,
    pane: crate::core::PaneId,
}

impl DaemonBridge {
    pub fn new(
        client: Arc<crate::daemon::client::DaemonClient>,
        pane: crate::core::PaneId,
    ) -> Self {
        Self { client, pane }
    }
}

#[async_trait]
impl PaneBridge for DaemonBridge {
    fn kind(&self) -> &'static str { "daemon" }

    async fn read(&self, buf: &mut [u8]) -> Result<usize> {
        // The daemon already applied the "wait up to 200 ms, then report
        // nothing" rule, so this is a plain copy-out.
        let bytes = self.client.read(self.pane, buf.len()).await?;
        let n = bytes.len().min(buf.len());
        buf[..n].copy_from_slice(&bytes[..n]);
        Ok(n)
    }

    async fn write(&self, data: &[u8]) -> Result<usize> {
        self.client.write(self.pane, data).await
    }

    async fn resize(&self, cols: u16, rows: u16) -> Result<()> {
        self.client.resize(self.pane, cols, rows).await
    }

    async fn close(&self) -> Result<()> {
        // P0-a: detach stops OUR pump. The daemon keeps the transport
        // (other panes / a later re-attach still need it) and keeps the
        // buffer (so the history is still readable when detached).
        self.client.detach(self.pane).await
    }
}

/// Tmux bridge: routes through the link's `TmuxSink`. We hold a `Weak`
/// reference to the sink (not `Arc`) so that the bridge doesn't keep
/// the sink alive after the link is dropped — when the sink's strong
/// refs are gone, the `Weak::upgrade` fails and the bridge returns
/// `Error::Sink("tmux sink dropped")` for any subsequent op.
#[derive(Debug)]
pub struct TmuxBridge {
    pub sink: std::sync::Weak<crate::sink::tmux::TmuxSink>,
    /// tmux pane handle — the "%N" string tmux assigns to a pane. All
    /// tmux CLI commands are addressed via this handle.
    pub handle: String,
}

impl TmuxBridge {
    fn sink(&self) -> Result<Arc<crate::sink::tmux::TmuxSink>> {
        self.sink
            .upgrade()
            .ok_or_else(|| crate::core::Error::Sink("tmux sink dropped".into()))
    }
}

#[async_trait]
impl PaneBridge for TmuxBridge {
    fn kind(&self) -> &'static str { "tmux" }

    async fn read(&self, buf: &mut [u8]) -> Result<usize> {
        self.sink()?.snapshot(&self.handle, buf).await
    }

    async fn write(&self, data: &[u8]) -> Result<usize> {
        self.sink()?.write(&self.handle, data).await
    }

    async fn resize(&self, cols: u16, rows: u16) -> Result<()> {
        self.sink()?.resize(&self.handle, cols, rows).await
    }

    async fn close(&self) -> Result<()> {
        // A tmux pane belongs to exactly one xscore pane: closing it is
        // a real `tmux kill-pane`. The link (session) survives.
        self.sink()?.close_pane(&self.handle).await
    }
}
