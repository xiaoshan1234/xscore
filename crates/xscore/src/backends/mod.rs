// backends/mod.rs
use crate::core::Result;
use crate::core::PaneId;
use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

#[async_trait]
pub trait PaneBackend: Send + Sync {
    async fn snapshot(&self, buf: &mut [u8]) -> Result<usize>;
    async fn write(&self, data: &[u8]) -> Result<usize>;
    async fn write_raw(&self, data: &[u8]) -> Result<usize>;
    async fn resize(&self, cols: u16, rows: u16) -> Result<()>;
    async fn close(&self) -> Result<()>;
}

#[derive(Default)]
pub struct BackendRegistry {
    inner: RwLock<HashMap<PaneId, Arc<dyn PaneBackend>>>,
}

impl BackendRegistry {
    pub fn new() -> Self { Self::default() }

    pub async fn insert(&self, id: PaneId, b: Arc<dyn PaneBackend>) {
        self.inner.write().await.insert(id, b);
    }

    pub async fn get(&self, id: PaneId) -> Option<Arc<dyn PaneBackend>> {
        self.inner.read().await.get(&id).cloned()
    }

    pub async fn remove(&self, id: PaneId) -> Option<Arc<dyn PaneBackend>> {
        self.inner.write().await.remove(&id)
    }
}

pub mod composite;
