use crate::core::Result;
use crate::transport::Transport;
use async_trait::async_trait;

pub struct RawSink;

#[async_trait]
impl super::Sink for RawSink {
    fn kind(&self) -> &'static str { "raw" }
    async fn attach(&mut self, _: &dyn Transport) -> Result<()> { Ok(()) }
    async fn new_pane(&self) -> Result<String> { Ok("raw-pane-0".into()) }
    async fn close_pane(&self, _: &str) -> Result<()> { Ok(()) }
    async fn list_panes(&self) -> Result<Vec<String>> { Ok(vec!["raw-pane-0".into()]) }
    async fn snapshot(&self, _: &str, _: &mut [u8]) -> Result<usize> { Ok(0) }
    async fn write(&self, _: &str, _: &[u8]) -> Result<usize> { Ok(0) }
    async fn resize(&self, _: &str, _: u16, _: u16) -> Result<()> { Ok(()) }
    async fn close(&self) -> Result<()> { Ok(()) }
}