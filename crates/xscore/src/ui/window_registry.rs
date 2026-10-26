// ui/window_registry.rs (v0.3.0 — window is independent, ADR 0007 v2)
//
// v0.3.0 changes:
//   - `by_link: HashMap<LinkId, Vec<WindowId>>` is REMOVED. Windows no
//     longer have a parent link — windows are independent entities.
//   - `WindowInfo.link_id` is REMOVED from the model too (see
//     `model::WindowInfo`).
//   - `list_for_link` is REMOVED. The CLI's `window list --link <id>`
//     is GONE — windows are not link-scoped anymore.
//
// What stays: `by_id` lookup, insert, remove. The `Window.panes`
// field still tracks the window's child panes (per `pane add-to-window`).
use super::window::Window;
use crate::ui::WindowId;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

type ArcWindow = Arc<Window>;

#[derive(Debug, Default)]
pub struct WindowRegistry {
    inner: RwLock<HashMap<WindowId, ArcWindow>>,
}

impl WindowRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub async fn insert(&self, window: Window) -> ArcWindow {
        let id = window.info.id;
        let arc = Arc::new(window);
        self.inner.write().await.insert(id, arc.clone());
        arc
    }

    pub async fn get(&self, id: WindowId) -> Option<ArcWindow> {
        self.inner.read().await.get(&id).cloned()
    }

    pub async fn list_all(&self) -> Vec<ArcWindow> {
        self.inner.read().await.values().cloned().collect()
    }

    pub async fn remove(&self, id: WindowId) -> Option<ArcWindow> {
        self.inner.write().await.remove(&id)
    }
}
