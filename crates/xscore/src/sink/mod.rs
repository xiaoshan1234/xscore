use crate::core::Result;
use crate::transport::Transport;
use async_trait::async_trait;

/// v0.3.0 (ADR 0007 v2): the Sink trait is the tmux-orchestration
/// layer for tmux-backed links. It is OPTIONAL — raw links use
/// `RawSink` (a no-op marker; tmux is bypassed entirely).
///
/// Windows are now independent entities (no link-owned windows), so
/// `new_window` / `kill_window` are REMOVED. `new_pane` is the only
/// way a sink allocates runtime resources, and it always splits
/// inside the link's session (which has a single tmux window by
/// default; v0.3.0 doesn't track per-window pane placement in tmux).
#[async_trait]
pub trait Sink: Send + Sync {
    fn kind(&self) -> &'static str;
    /// Wire this sink to a transport. Called once per link lifetime.
    /// For tmux-backed links: opens the tmux session (which opens the
    /// transport first if needed). For raw links: no-op.
    async fn attach(&self, _transport: &dyn Transport) -> Result<()>;

    /// Create a new tmux split inside the link's session. Returns the
    /// tmux pane handle (e.g. "%7"). Each call allocates a NEW split;
    /// a tmux-backed link can host N simultaneous panes (Q1 corollary).
    async fn new_pane(&self) -> Result<String>;

    async fn close_pane(&self, pane_handle: &str) -> Result<()>;
    async fn list_panes(&self) -> Result<Vec<String>>;
    async fn snapshot(&self, pane_handle: &str, buf: &mut [u8]) -> Result<usize>;
    async fn write(&self, pane_handle: &str, data: &[u8]) -> Result<usize>;
    async fn resize(&self, pane_handle: &str, cols: u16, rows: u16) -> Result<()>;
    async fn close(&self) -> Result<()>;
}

pub mod factory;
pub mod raw;
pub mod tmux;

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use crate::transport::Transport;

    struct Noop;
    #[async_trait]
    impl Sink for Noop {
        fn kind(&self) -> &'static str { "noop" }
        async fn attach(&self, _: &dyn Transport) -> Result<()> { Ok(()) }
        async fn new_pane(&self) -> Result<String> { Ok("pane-stub".into()) }
        async fn close_pane(&self, _: &str) -> Result<()> { Ok(()) }
        async fn list_panes(&self) -> Result<Vec<String>> { Ok(vec![]) }
        async fn snapshot(&self, _: &str, _: &mut [u8]) -> Result<usize> { Ok(0) }
        async fn write(&self, _: &str, data: &[u8]) -> Result<usize> { Ok(data.len()) }
        async fn resize(&self, _: &str, _: u16, _: u16) -> Result<()> { Ok(()) }
        async fn close(&self) -> Result<()> { Ok(()) }
    }

    #[tokio::test]
    async fn noop_sink_creates_and_closes_pane() {
        let s = Noop;
        let p = s.new_pane().await.unwrap();
        assert_eq!(p, "pane-stub");
        s.close_pane(&p).await.unwrap();
    }
}