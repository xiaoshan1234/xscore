// backends/composite.rs
use crate::core::link::ArcLink;
use crate::core::PaneId;
use crate::core::Result;
use async_trait::async_trait;

pub struct CompositeBackend {
    #[allow(dead_code)] // reserved for future per-pane bookkeeping (task-06 follow-up)
    pane_id: PaneId,
    link: ArcLink,
    sink_handle: String,
}

impl CompositeBackend {
    pub fn new(pane_id: PaneId, link: ArcLink, sink_handle: String) -> Self {
        Self { pane_id, link, sink_handle }
    }
}

#[async_trait]
impl super::PaneBackend for CompositeBackend {
    async fn snapshot(&self, buf: &mut [u8]) -> Result<usize> {
        self.link.sink.snapshot(&self.sink_handle, buf).await
    }
    async fn write(&self, data: &[u8]) -> Result<usize> {
        self.link.sink.write(&self.sink_handle, data).await
    }
    async fn write_raw(&self, data: &[u8]) -> Result<usize> {
        self.link.transport.write(data).await
    }
    async fn resize(&self, cols: u16, rows: u16) -> Result<()> {
        self.link.transport.resize(cols, rows).await?;
        self.link.sink.resize(&self.sink_handle, cols, rows).await?;
        Ok(())
    }
    async fn close(&self) -> Result<()> {
        // close the pane only — link survives
        self.link.sink.close_pane(&self.sink_handle).await
    }
}
