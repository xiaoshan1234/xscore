// core/window.rs (FINAL — no `placeholder` stub; parent writes manager.rs
// against this surface).
use tokio::sync::Mutex;
use super::id::PaneId;
use crate::model::WindowInfo;

#[derive(Debug)]
pub struct Window {
    pub info: WindowInfo,
    pub panes: Mutex<Vec<PaneId>>,
}

impl Window {
    pub fn new(info: WindowInfo) -> Self {
        Self { info, panes: Mutex::new(Vec::new()) }
    }

    pub async fn add_pane(&self, pane_id: PaneId) {
        self.panes.lock().await.push(pane_id);
    }

    pub async fn remove_pane(&self, pane_id: PaneId) {
        self.panes.lock().await.retain(|p| *p != pane_id);
    }
}
