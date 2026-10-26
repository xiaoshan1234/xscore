// core/link_registry.rs
use super::address::LinkAddress;
use super::id::LinkId;
use super::link::{ArcLink, Link};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

#[derive(Default)]
pub struct LinkRegistry {
    inner: RwLock<LinkRegistryInner>,
}

#[derive(Default)]
struct LinkRegistryInner {
    by_id: HashMap<LinkId, ArcLink>,
    by_address: HashMap<LinkAddress, LinkId>,
}

impl LinkRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub async fn insert(&self, link: Link) -> ArcLink {
        let id = link.id;
        let address = link.info.address.clone();
        let arc = Arc::new(link);
        let mut g = self.inner.write().await;
        g.by_address.insert(address, id);
        g.by_id.insert(id, arc.clone());
        arc
    }

    pub async fn get(&self, id: LinkId) -> Option<ArcLink> {
        self.inner.read().await.by_id.get(&id).cloned()
    }

    pub async fn find_by_address(&self, address: &LinkAddress) -> Option<ArcLink> {
        let g = self.inner.read().await;
        let id = g.by_address.get(address).copied()?;
        g.by_id.get(&id).cloned()
    }

    pub async fn list(&self) -> Vec<ArcLink> {
        self.inner.read().await.by_id.values().cloned().collect()
    }

    pub async fn remove(&self, id: LinkId) -> Option<ArcLink> {
        let mut g = self.inner.write().await;
        let arc = g.by_id.remove(&id)?;
        let info = &arc.info;
        let addr = info.address.clone();
        // only delete from by_address if still pointing at this id (id may have been re-inserted)
        if g.by_address.get(&addr) == Some(&id) {
            g.by_address.remove(&addr);
        }
        Some(arc)
    }
}
