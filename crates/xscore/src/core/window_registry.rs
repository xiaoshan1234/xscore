// core/window_registry.rs
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use crate::core::{LinkId, WindowId};
use super::window::Window;

type ArcWindow = Arc<Window>;

#[derive(Debug, Default)]
pub struct WindowRegistryInner {
    by_id: HashMap<WindowId, ArcWindow>,
    by_link: HashMap<LinkId, Vec<WindowId>>,
}

#[derive(Debug, Default)]
pub struct WindowRegistry {
    inner: RwLock<WindowRegistryInner>,
}

impl WindowRegistry {
    pub fn new() -> Self { Self::default() }

    pub async fn insert(&self, window: Window) -> ArcWindow {
        let mut g = self.inner.write().await;
        let link_id = window.info.link_id;
        let id = window.info.id;
        let entry = g.by_link.entry(link_id).or_default();
        if !entry.contains(&id) { entry.push(id); }
        let arc = Arc::new(window);
        g.by_id.insert(id, arc.clone());
        arc
    }

    pub async fn get(&self, id: WindowId) -> Option<ArcWindow> {
        self.inner.read().await.by_id.get(&id).cloned()
    }

    pub async fn list_for_link(&self, link_id: LinkId) -> Vec<ArcWindow> {
        let g = self.inner.read().await;
        let ids = g.by_link.get(&link_id).cloned().unwrap_or_default();
        ids.iter().filter_map(|wid| g.by_id.get(wid).cloned()).collect()
    }

    pub async fn remove(&self, id: WindowId) -> Option<ArcWindow> {
        let mut g = self.inner.write().await;
        let arc = g.by_id.remove(&id)?;
        if let Some(ids) = g.by_link.get_mut(&arc.info.link_id) {
            ids.retain(|w| *w != id);
        }
        Some(arc)
    }
}
