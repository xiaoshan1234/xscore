// backends/bridge.rs
//
// v0.3.0 — `PaneBridge` trait + `DirectBridge` and `TmuxBridge` impls.
//
// The bridge is the per-pane runtime that exposes `read` / `write` /
// `resize` / `close`. Two concrete kinds:
//
//   - `DirectBridge`: wraps the link's `Arc<dyn Transport>` directly.
//     `read` is a one-shot (whatever bytes are immediately available).
//     `write` is forward-and-forget. No history (a local ring buffer is
//     deferred to v0.4.0+).
//
//   - `TmuxBridge`: wraps the link's `TmuxSink` (via `Weak<TmuxSink>` to
//     avoid reference cycles). `read` calls `tmux capture-pane` (returns
//     tmux's scrollback). `write` calls `tmux send-keys`. `resize` calls
//     `tmux resize-pane`. `close` calls `tmux kill-pane`.
//
// Each pane holds `Mutex<Option<Arc<dyn PaneBridge>>>`. `None` means the
// pane is orphan (no link attached). Setting the bridge is what
// `xscore pane attach` does; clearing it is what `xscore pane detach`
// does. The pane keeps its buffer content (in tmux's scrollback or in
// whatever the transport has buffered); the bridge is just the I/O
// handle.

use std::sync::Arc;
use async_trait::async_trait;

use crate::core::Result;
use crate::sink::Sink;
use crate::transport::Transport;

/// Per-pane runtime bridge. Object-safe (`dyn PaneBridge` is valid)
/// because every method takes `&self` and the trait has no associated
/// types. `Send + Sync` is required so the bridge can be shared into
/// background tasks (snapshot/writer pump) and into the data CLI.
#[async_trait]
pub trait PaneBridge: Send + Sync + std::fmt::Debug {
    /// What kind of bridge this is — `"direct"` or `"tmux"`. Used for
    /// diagnostics and (later) for read-write mode switching (e.g. tmux
    /// bridges want capture-pane while raw bridges need a ring buffer).
    fn kind(&self) -> &'static str;

    /// Pull bytes from the bridge into `buf`. Returns the number of
    /// bytes read (0 means "no data available right now"). For tmux
    /// bridges this is a `tmux capture-pane` snapshot (one frame); for
    /// raw bridges it's a one-shot `transport.read(buf)` (whatever the
    /// transport has buffered).
    async fn read(&self, buf: &mut [u8]) -> Result<usize>;

    /// Push bytes into the bridge. Returns the number of bytes
    /// accepted (some sinks may do partial writes if their internal
    /// buffer is full).
    async fn write(&self, data: &[u8]) -> Result<usize>;

    /// Resize the pane's terminal dimensions. For tmux bridges this
    /// also resizes the transport (so the remote shell sees the new
    /// size). For raw bridges only the transport is resized.
    async fn resize(&self, cols: u16, rows: u16) -> Result<()>;

    /// Close the bridge (kills the tmux split-pane, or closes the
    /// transport). The link survives — only this pane's I/O handle
    /// goes away.
    async fn close(&self) -> Result<()>;
}

/// Direct bridge over the link's `Arc<dyn Transport>`. Used when the
/// link's sink is `RawSink` (default — no tmux). One direct bridge per
/// attached pane; the transport is shared across all direct bridges on
/// the same link (via `Arc`).
#[derive(Debug)]
pub struct DirectBridge {
    pub transport: Arc<dyn Transport>,
}

#[async_trait]
impl PaneBridge for DirectBridge {
    fn kind(&self) -> &'static str { "direct" }

    async fn read(&self, buf: &mut [u8]) -> Result<usize> {
        // One-shot: whatever bytes the transport has right now. A ring
        // buffer sidecar (so detached panes keep their history) is
        // deferred to v0.4.0+ per ADR 0007 v2.
        self.transport.read(buf).await
    }

    async fn write(&self, data: &[u8]) -> Result<usize> {
        self.transport.write(data).await
    }

    async fn resize(&self, cols: u16, rows: u16) -> Result<()> {
        self.transport.resize(cols, rows).await
    }

    async fn close(&self) -> Result<()> {
        // Closing one direct bridge does NOT close the link's transport
        // — other panes may still be using it. The transport closes
        // when the link itself is deleted.
        Ok(())
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
    /// tmux pane handle — the "%N" string tmux assigns to a split.
    /// All tmux CLI commands are addressed via this handle.
    pub handle: String,
}

#[async_trait]
impl PaneBridge for TmuxBridge {
    fn kind(&self) -> &'static str { "tmux" }

    async fn read(&self, buf: &mut [u8]) -> Result<usize> {
        let sink = self.sink.upgrade()
            .ok_or_else(|| crate::core::Error::Sink("tmux sink dropped".into()))?;
        sink.snapshot(&self.handle, buf).await
    }

    async fn write(&self, data: &[u8]) -> Result<usize> {
        let sink = self.sink.upgrade()
            .ok_or_else(|| crate::core::Error::Sink("tmux sink dropped".into()))?;
        sink.write(&self.handle, data).await
    }

    async fn resize(&self, cols: u16, rows: u16) -> Result<()> {
        let sink = self.sink.upgrade()
            .ok_or_else(|| crate::core::Error::Sink("tmux sink dropped".into()))?;
        sink.resize(&self.handle, cols, rows).await
    }

    async fn close(&self) -> Result<()> {
        let sink = self.sink.upgrade()
            .ok_or_else(|| crate::core::Error::Sink("tmux sink dropped".into()))?;
        // close = kill-pane -t <handle>. The link survives.
        sink.close_pane(&self.handle).await
    }
}