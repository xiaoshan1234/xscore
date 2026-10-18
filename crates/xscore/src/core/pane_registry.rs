// core/pane_registry.rs
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use super::id::{LinkId, PaneId};
use crate::model::PaneInfo;

#[derive(Default)]
pub struct PaneRegistry {
    inner: RwLock<HashMap<PaneId, PaneInfo>>,
}

impl PaneRegistry {
    pub fn new() -> Self { Self::default() }

    pub async fn insert(&self, info: PaneInfo) {
        self.inner.write().await.insert(info.id, info);
    }

    pub async fn get(&self, id: PaneId) -> Option<PaneInfo> {
        self.inner.read().await.get(&id).cloned()
    }

    pub async fn remove(&self, id: PaneId) -> Option<PaneInfo> {
        self.inner.write().await.remove(&id)
    }

    pub async fn list_for_link(&self, _link_id: LinkId) -> Vec<PaneInfo> {
        // Note: after the task-14 rename `PaneInfo.link_id` → `window_id`,
        // filtering by parent link requires a join through WindowRegistry.
        // That wiring lands in task-14's manager.rs integration glue; for
        // this slice we return all panes so the registry still compiles.
        self.inner.read().await.values().cloned().collect()
    }

    pub async fn list_all(&self) -> Vec<PaneInfo> {
        self.inner.read().await.values().cloned().collect()
    }
}

pub type ArcPaneRegistry = Arc<PaneRegistry>;
