// core/pane_registry.rs (v0.3.0 — buffer-per-pane, ADR 0007 v2)
//
// v0.3.0: PaneRegistry tracks ONLY `PaneInfo` (the JSON-serializable
// fields). The bridge (live I/O handle) is held in the manager's
// `pane_bridges` map separately. This is necessary because mutating
// `PaneInfo` would otherwise destroy the bridge (Pane has both fields
// and `Pane::new(info)` resets the bridge).
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use super::id::PaneId;
use crate::model::PaneInfo;

#[derive(Default)]
pub struct PaneRegistry {
    inner: RwLock<HashMap<PaneId, PaneInfo>>,
}

impl PaneRegistry {
    pub fn new() -> Self { Self::default() }

    /// Insert or update a pane's info (in-place — preserves any
    /// bridge the manager has associated with this pane id).
    pub async fn insert(&self, info: PaneInfo) {
        self.inner.write().await.insert(info.id, info);
    }

    pub async fn get(&self, id: PaneId) -> Option<PaneInfo> {
        self.inner.read().await.get(&id).cloned()
    }

    pub async fn remove(&self, id: PaneId) -> Option<PaneInfo> {
        self.inner.write().await.remove(&id)
    }

    pub async fn list_all(&self) -> Vec<PaneInfo> {
        self.inner.read().await.values().cloned().collect()
    }
}

// keep Arc import live for downstream consumers
#[allow(dead_code)]
fn _arc<T>(_x: Arc<T>) {}