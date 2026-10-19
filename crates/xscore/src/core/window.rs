// core/window.rs (v0.3.0 — pure UI layout container, ADR 0007 v2)
//
// Window is a pure UI layout container. No `link_id` field — windows
// are independent entities that may exist with zero panes and may be
// orphan (no link relationship).
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