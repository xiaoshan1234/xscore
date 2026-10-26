//! Buffer model and registry.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;

use crate::service::link_service::LinkId;
use crate::service::session_service::SessionId;

uuid_id!(BufferId);

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct BufferInfo {
    pub id: BufferId,
    pub session_id: SessionId,
    pub name: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub origin: BufferOrigin,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum BufferOrigin {
    Link {
        link_id: LinkId,
    },
    Tmux {
        link_id: LinkId,
        pane_handle: String,
    },
}

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn buffer_id_roundtrip_via_string() {
        let id = BufferId::new();
        assert_eq!(BufferId::parse(&id.to_string()).unwrap(), id);
    }
}
