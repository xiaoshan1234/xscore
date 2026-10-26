//! Session runtime row and registry.

use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::RwLock;

use super::model::{SessionId, SessionInfo};

#[derive(Debug)]
pub struct Session {
    pub info: SessionInfo,
}

impl Session {
    pub fn new(info: SessionInfo) -> Self {
        Self { info }
    }
}

pub type ArcSession = Arc<Session>;

#[derive(Default)]
pub struct SessionRegistry {
    inner: RwLock<SessionRegistryInner>,
}

#[derive(Default)]
struct SessionRegistryInner {
    by_id: HashMap<SessionId, ArcSession>,
    by_name: HashMap<String, SessionId>,
}

impl SessionRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub async fn insert(&self, session: Session) -> ArcSession {
        let id = session.info.id;
        let name = session.info.name.clone();
        let arc = Arc::new(session);
        let mut guard = self.inner.write().await;
        guard.by_name.insert(name, id);
        guard.by_id.insert(id, Arc::clone(&arc));
        arc
    }

    pub async fn get(&self, id: SessionId) -> Option<ArcSession> {
        self.inner.read().await.by_id.get(&id).cloned()
    }

    pub async fn get_by_name(&self, name: &str) -> Option<ArcSession> {
        let guard = self.inner.read().await;
        let id = guard.by_name.get(name).copied()?;
        guard.by_id.get(&id).cloned()
    }

    pub async fn list(&self) -> Vec<ArcSession> {
        self.inner.read().await.by_id.values().cloned().collect()
    }

    pub async fn remove(&self, id: SessionId) -> Option<ArcSession> {
        let mut guard = self.inner.write().await;
        let arc = guard.by_id.remove(&id)?;
        let name = &arc.info.name;
        if guard.by_name.get(name) == Some(&id) {
            guard.by_name.remove(name);
        }
        Some(arc)
    }
}
