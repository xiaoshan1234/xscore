use std::collections::HashMap;

use tokio::sync::RwLock;

use crate::domain::model::BufferInfo;
use crate::domain::BufferId;

#[derive(Default)]
pub struct BufferRegistry {
    inner: RwLock<HashMap<BufferId, BufferInfo>>,
}

impl BufferRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub async fn insert(&self, info: BufferInfo) {
        self.inner.write().await.insert(info.id, info);
    }

    pub async fn get(&self, id: BufferId) -> Option<BufferInfo> {
        self.inner.read().await.get(&id).cloned()
    }

    pub async fn remove(&self, id: BufferId) -> Option<BufferInfo> {
        self.inner.write().await.remove(&id)
    }

    pub async fn list_all(&self) -> Vec<BufferInfo> {
        self.inner.read().await.values().cloned().collect()
    }
}
