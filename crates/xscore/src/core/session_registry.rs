// core/session_registry.rs (v0.6.0 — ADR 0010 §1)
//
// Mirrors `LinkRegistry`: `by_id` for identity, `by_name` for the user-facing
// handle (`session show work`). Both maps are kept under one lock so a rename
// can never leave `by_name` pointing at a stale id.
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

use super::id::SessionId;
use super::session::Session;

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
    pub fn new() -> Self { Self::default() }

    /// Insert (or replace) a session. The name index follows the new row, so
    /// re-inserting an edited `SessionInfo` is safe — that is what makes
    /// `rename_session` a remove + insert without a second code path.
    pub async fn insert(&self, session: Session) -> ArcSession {
        let id = session.info.id;
        let name = session.info.name.clone();
        let arc = Arc::new(session);
        let mut g = self.inner.write().await;
        g.by_name.insert(name, id);
        g.by_id.insert(id, arc.clone());
        arc
    }

    pub async fn get(&self, id: SessionId) -> Option<ArcSession> {
        self.inner.read().await.by_id.get(&id).cloned()
    }

    pub async fn get_by_name(&self, name: &str) -> Option<ArcSession> {
        let g = self.inner.read().await;
        let id = g.by_name.get(name).copied()?;
        g.by_id.get(&id).cloned()
    }

    pub async fn list(&self) -> Vec<ArcSession> {
        self.inner.read().await.by_id.values().cloned().collect()
    }

    pub async fn remove(&self, id: SessionId) -> Option<ArcSession> {
        let mut g = self.inner.write().await;
        let arc = g.by_id.remove(&id)?;
        let name = &arc.info.name;
        // Only drop the name index if it still points at this id (the name may
        // have been re-registered by a rename or a re-insert).
        if g.by_name.get(name) == Some(&id) {
            g.by_name.remove(name);
        }
        Some(arc)
    }
}
