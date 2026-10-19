use crate::core::Result;
use crate::transport::Transport;
use async_trait::async_trait;

/// No-op sink for raw links (the v0.3.0 default). The link's `data`
/// ops go through `transport` directly via `DirectBridge`. This
/// sink exists so the link has a `Box<dyn Sink>` to match the
/// `Link` struct shape — it's never actually invoked.
pub struct RawSink;

#[async_trait]
impl super::Sink for RawSink {
    fn kind(&self) -> &'static str { "raw" }
    async fn attach(&self, _: &dyn Transport) -> Result<()> { Ok(()) }
    async fn new_pane(&self) -> Result<String> { Ok("raw-pane-0".into()) }
    async fn close_pane(&self, _: &str) -> Result<()> { Ok(()) }
    async fn list_panes(&self) -> Result<Vec<String>> { Ok(vec!["raw-pane-0".into()]) }
    async fn snapshot(&self, _: &str, _: &mut [u8]) -> Result<usize> { Ok(0) }
    async fn write(&self, _: &str, _: &[u8]) -> Result<usize> { Ok(0) }
    async fn resize(&self, _: &str, _: u16, _: u16) -> Result<()> { Ok(()) }
    async fn close(&self) -> Result<()> { Ok(()) }
}