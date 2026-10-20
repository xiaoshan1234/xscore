use crate::core::{Error, Result};
use crate::transport::Transport;
use async_trait::async_trait;

use super::{TmuxPaneSnapshot, TmuxWindowSnapshot};

/// No-op sink for raw links (the default). The link's `data` ops go
/// through `transport` directly via `DirectBridge`. It has no tmux
/// windows or panes at all, so every structural query is a hard error
/// rather than a fabricated empty answer (v0.4.0, ADR 0008).
pub struct RawSink;

fn no_tmux() -> Error {
    Error::Sink("raw sink has no tmux windows or panes".into())
}

#[async_trait]
impl super::Sink for RawSink {
    fn kind(&self) -> &'static str { "raw" }

    async fn attach(&self, _: &dyn Transport) -> Result<()> { Ok(()) }

    async fn list_windows(&self) -> Result<Vec<TmuxWindowSnapshot>> { Err(no_tmux()) }
    async fn list_panes(&self, _: &str) -> Result<Vec<TmuxPaneSnapshot>> { Err(no_tmux()) }
    async fn new_window(&self, _: &str) -> Result<TmuxWindowSnapshot> { Err(no_tmux()) }
    async fn kill_window(&self, _: &str) -> Result<()> { Err(no_tmux()) }
    async fn split_pane(&self, _: &str) -> Result<TmuxPaneSnapshot> { Err(no_tmux()) }

    async fn close_pane(&self, _: &str) -> Result<()> { Ok(()) }
    async fn snapshot(&self, _: &str, _: &mut [u8]) -> Result<usize> { Ok(0) }
    async fn write(&self, _: &str, _: &[u8]) -> Result<usize> { Ok(0) }
    async fn resize(&self, _: &str, _: u16, _: u16) -> Result<()> { Ok(()) }
    async fn close(&self) -> Result<()> { Ok(()) }
}
