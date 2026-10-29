//! Buffer lifecycle, ring storage and runtime dispatch used by UI panes.

use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::Mutex;

use crate::service::session_service::SessionId;
use crate::{Error, Result};

pub mod ring;
pub(crate) mod runtime;
pub mod store;

pub use ring::{RingBuffer, DIRECT_BUFFER_CAPACITY};
pub(crate) use runtime::{fresh_ring, BufferRuntime};
pub use store::{BufferId, BufferInfo, BufferOrigin, BufferRegistry};

#[derive(Clone, Default)]
pub struct BufferService {
    buffers: Arc<BufferRegistry>,
    runtimes: Arc<Mutex<HashMap<BufferId, Arc<BufferRuntime>>>>,
    memory: Arc<Mutex<HashMap<BufferId, Arc<Mutex<RingBuffer>>>>>,
}

impl BufferService {
    pub fn new() -> Self {
        Self::default()
    }

    pub async fn get(&self, id: BufferId) -> Option<BufferInfo> {
        self.buffers.get(id).await
    }

    pub async fn list(&self) -> Vec<BufferInfo> {
        self.buffers.list_all().await
    }

    pub async fn insert(&self, info: BufferInfo) {
        self.buffers.insert(info).await;
    }

    pub async fn remove(&self, id: BufferId) -> Option<BufferInfo> {
        self.buffers.remove(id).await
    }

    pub(crate) async fn runtime(&self, id: BufferId) -> Option<Arc<BufferRuntime>> {
        self.runtimes.lock().await.get(&id).cloned()
    }

    pub(crate) async fn put_runtime(&self, id: BufferId, runtime: Arc<BufferRuntime>) {
        self.runtimes.lock().await.insert(id, runtime);
    }

    pub(crate) async fn take_runtime(&self, id: BufferId) -> Option<Arc<BufferRuntime>> {
        self.runtimes.lock().await.remove(&id)
    }

    pub(crate) async fn stop_runtime(&self, id: BufferId) {
        if let Some(runtime) = self.take_runtime(id).await {
            let _ = runtime.close().await;
        }
    }

    pub(crate) async fn forget(&self, id: BufferId) {
        self.stop_runtime(id).await;
        self.remove_memory(id).await;
        self.remove(id).await;
    }

    pub(crate) async fn ring(&self, id: BufferId) -> Arc<Mutex<RingBuffer>> {
        let mut memory = self.memory.lock().await;
        memory.entry(id).or_insert_with(fresh_ring).clone()
    }

    pub(crate) async fn remove_memory(&self, id: BufferId) {
        self.memory.lock().await.remove(&id);
    }

    pub(crate) async fn create(
        &self,
        session_id: SessionId,
        name: String,
        origin: BufferOrigin,
    ) -> BufferId {
        let id = BufferId::new();
        self.buffers
            .insert(BufferInfo {
                id,
                session_id,
                name,
                created_at: chrono::Utc::now(),
                origin,
            })
            .await;
        id
    }

    pub(crate) async fn list_for_session(&self, session_id: SessionId) -> Vec<BufferInfo> {
        self.buffers
            .list_all()
            .await
            .into_iter()
            .filter(|buffer| buffer.session_id == session_id)
            .collect()
    }

    pub(crate) async fn forget_all_for_session(&self, session_id: SessionId) {
        for buffer in self.list_for_session(session_id).await {
            self.forget(buffer.id).await;
        }
    }

    pub(crate) async fn read(&self, id: BufferId, out: &mut [u8]) -> Result<usize> {
        self.runtime_required(id).await?.read(out).await
    }

    pub(crate) async fn write(&self, id: BufferId, data: &[u8]) -> Result<usize> {
        self.runtime_required(id).await?.write(data).await
    }

    pub(crate) async fn resize(&self, id: BufferId, cols: u16, rows: u16) -> Result<()> {
        self.runtime_required(id).await?.resize(cols, rows).await
    }

    async fn runtime_required(&self, id: BufferId) -> Result<Arc<BufferRuntime>> {
        self.runtime(id)
            .await
            .ok_or_else(|| Error::Invalid(format!("buffer {id} has no runtime")))
    }
}
