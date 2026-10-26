//! Link model, live runtime row and registry.

use std::collections::HashMap;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;

use crate::infra::link::{LinkAddress, Transport, TransportConfig};
use crate::infra::tmux::TmuxControl;
use crate::service::session_service::SessionId;

uuid_id!(LinkId);

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct LinkInfo {
    pub id: LinkId,
    pub session_id: SessionId,
    pub name: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub address: LinkAddress,
    pub endpoint: EndpointRef,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum EndpointRef {
    Pipe(TransportConfig),
    Control,
}

impl LinkInfo {
    pub fn transport_str(&self) -> &'static str {
        self.address.kind()
    }

    pub fn is_control(&self) -> bool {
        matches!(self.endpoint, EndpointRef::Control)
    }
}

pub struct Link {
    pub id: LinkId,
    pub info: LinkInfo,
    pub transport: Arc<dyn Transport>,
    pub control: Option<Arc<TmuxControl>>,
    pub connected: bool,
}

pub type ArcLink = Arc<Link>;

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
        let mut guard = self.inner.write().await;
        guard.by_address.insert(address, id);
        guard.by_id.insert(id, Arc::clone(&arc));
        arc
    }

    pub async fn get(&self, id: LinkId) -> Option<ArcLink> {
        self.inner.read().await.by_id.get(&id).cloned()
    }

    pub async fn find_by_address(&self, address: &LinkAddress) -> Option<ArcLink> {
        let guard = self.inner.read().await;
        let id = guard.by_address.get(address).copied()?;
        guard.by_id.get(&id).cloned()
    }

    pub async fn list(&self) -> Vec<ArcLink> {
        self.inner.read().await.by_id.values().cloned().collect()
    }

    pub async fn remove(&self, id: LinkId) -> Option<ArcLink> {
        let mut guard = self.inner.write().await;
        let arc = guard.by_id.remove(&id)?;
        let address = arc.info.address.clone();
        if guard.by_address.get(&address) == Some(&id) {
            guard.by_address.remove(&address);
        }
        Some(arc)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn link_id_roundtrip_via_string() {
        let id = LinkId::new();
        assert_eq!(LinkId::parse(&id.to_string()).unwrap(), id);
    }
}
