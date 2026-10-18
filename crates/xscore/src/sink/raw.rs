use crate::core::Result;
use crate::transport::Transport;
use async_trait::async_trait;

pub struct RawSink;

#[async_trait]
impl super::Sink for RawSink {
    fn kind(&self) -> &'static str { "raw" }
    async fn attach(&self, _: &dyn Transport) -> Result<()> { Ok(()) }
    async fn new_window(&self, _name: &str) -> Result<crate::model::WindowInfo> {
        // Noop — for ?tmux=off path. Returns a placeholder WindowInfo.
        // The manager handles the in-memory bookkeeping; the sink just
        // doesn't allocate a tmux window.
        unreachable!("raw_sink::new_window not implemented (sink is a no-op for ?tmux=off)")
    }
    async fn kill_window(&self, _idx: u32) -> Result<()> { Ok(()) }
    async fn new_pane(&self, _window_index: u32) -> Result<String> { Ok("raw-pane-0".into()) }
    async fn close_pane(&self, _: &str) -> Result<()> { Ok(()) }
    async fn list_panes(&self) -> Result<Vec<String>> { Ok(vec!["raw-pane-0".into()]) }
    async fn snapshot(&self, _: &str, _: &mut [u8]) -> Result<usize> { Ok(0) }
    async fn write(&self, _: &str, _: &[u8]) -> Result<usize> { Ok(0) }
    async fn resize(&self, _: &str, _: u16, _: u16) -> Result<()> { Ok(()) }
    async fn close(&self) -> Result<()> { Ok(()) }
}