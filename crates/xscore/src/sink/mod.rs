use crate::core::Result;
use crate::transport::Transport;
use async_trait::async_trait;

#[async_trait]
pub trait Sink: Send + Sync {
    fn kind(&self) -> &'static str;
    /// Wire this sink to a transport. Called once per link lifetime.
    async fn attach(&self, _transport: &dyn Transport) -> Result<()>;

    /// NEW (ADR 0006): create a new tmux window inside the sink's session.
    /// Returns the WindowInfo; the sink retains the address.
    async fn new_window(&self, name: &str) -> Result<crate::model::WindowInfo>;

    /// NEW (ADR 0006): kill a window by its tmux-assigned index. Panes
    /// inside die with it.
    async fn kill_window(&self, tmux_window_index: u32) -> Result<()>;

    /// ADR 0006: pane is a tmux split inside a window (was: a tmux window).
    /// Takes a window index; returns the tmux pane_id of the new split.
    async fn new_pane(&self, tmux_window_index: u32) -> Result<String>;

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
        async fn new_window(&self, _: &str) -> Result<crate::model::WindowInfo> {
            // Noop doesn't allocate WindowIds. The test never calls this —
            // keeping the signature satisfied so the trait stays object-safe.
            unreachable!("noop_sink::new_window not used in test")
        }
        async fn kill_window(&self, _: u32) -> Result<()> { Ok(()) }
        async fn new_pane(&self, _: u32) -> Result<String> { Ok("pane-stub".into()) }
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
        let p = s.new_pane(0).await.unwrap();
        assert_eq!(p, "pane-stub");
        s.close_pane(&p).await.unwrap();
    }
}