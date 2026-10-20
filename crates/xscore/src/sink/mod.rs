// sink/mod.rs (v0.4.0 — tmux adoption, ADR 0008)
//
// The `Sink` trait is the tmux-orchestration layer for tmux-backed
// links. It is OPTIONAL — raw links use `RawSink` (a no-op marker; tmux
// is bypassed entirely).
//
// v0.4.0 changes (ADR 0008):
//   - tmux is the SOURCE OF TRUTH for structure. The sink reports what
//     tmux actually has (`list_windows`, `list_panes`) instead of xscore
//     predicting PaneId/WindowId shapes. `new_window` and `split_pane`
//     return *snapshots with geometry*, so the caller never has to guess
//     `%N`/`@N` or cols/rows.
//   - `new_pane()` and the session-wide string `list_panes()` are gone:
//     panes are created inside a specific tmux window (`split_pane`) and
//     identified by their `%N` handle.
//   - Identity anchors are `%N` (pane) and `@N` (window). The numeric
//     *index* is display-only: it shifts as windows/panes come and go.
use crate::core::Result;
use crate::model::WindowLayout;
use crate::transport::Transport;
use async_trait::async_trait;

/// A tmux window as tmux describes it, plus the geometry xscore stores.
#[derive(Debug, Clone)]
pub struct TmuxWindowSnapshot {
    pub handle: String,   // "@0"
    pub name: String,
    pub cols: u16,
    pub rows: u16,
    pub layout: String,   // raw tmux layout string
}

/// A tmux pane as tmux describes it, plus its geometry.
#[derive(Debug, Clone)]
pub struct TmuxPaneSnapshot {
    pub handle: String,   // "%3"
    pub cols: u16,
    pub rows: u16,
    pub x: u16,           // pane_left
    pub y: u16,           // pane_top
    pub index: u32,       // pane_index (display-only)
}

/// Best-effort translation of tmux's raw layout string into the
/// `WindowLayout` hint. Unparseable layouts fall back to `Single` — the
/// per-pane geometry is always authoritative for actual placement.
pub fn layout_from_tmux(raw: &str) -> WindowLayout {
    if raw.contains("even-horizontal") {
        WindowLayout::SplitHorizontal
    } else if raw.contains("even-vertical") {
        WindowLayout::SplitVertical
    } else if raw.contains("tiled") {
        WindowLayout::Grid { cols: 0, rows: 0 }
    } else {
        WindowLayout::Single
    }
}

#[async_trait]
pub trait Sink: Send + Sync {
    fn kind(&self) -> &'static str;

    /// Wire this sink to a transport. Called once per link lifetime.
    /// For tmux-backed links: opens the tmux session (which opens the
    /// transport first if needed). For raw links: no-op.
    async fn attach(&self, _transport: &dyn Transport) -> Result<()>;

    // ----- adoption (v0.4.0): ask tmux, never predict -----
    async fn list_windows(&self) -> Result<Vec<TmuxWindowSnapshot>>;
    async fn list_panes(&self, window_handle: &str) -> Result<Vec<TmuxPaneSnapshot>>;

    // ----- tmux window lifecycle -----
    async fn new_window(&self, name: &str) -> Result<TmuxWindowSnapshot>;
    async fn kill_window(&self, window_handle: &str) -> Result<()>;

    /// Split inside a SPECIFIC tmux window (there is no implicit window
    /// 0) and return the new pane together with its real geometry.
    async fn split_pane(&self, window_handle: &str) -> Result<TmuxPaneSnapshot>;

    async fn close_pane(&self, pane_handle: &str) -> Result<()>;
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

    #[test]
    fn layout_parsing_maps_tmux_strings() {
        assert!(matches!(layout_from_tmux("even-horizontal"), WindowLayout::SplitHorizontal));
        assert!(matches!(layout_from_tmux("even-vertical"), WindowLayout::SplitVertical));
        assert!(matches!(layout_from_tmux("tiled"), WindowLayout::Grid { .. }));
        assert!(matches!(
            layout_from_tmux("bb62,120x30,0,0[120x14,0,0,1,120x15,0,15,2]"),
            WindowLayout::Single
        ));
    }
}
