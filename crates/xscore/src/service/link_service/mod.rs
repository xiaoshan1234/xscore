//! Link lifecycle and transport ownership.

use std::sync::Arc;

pub mod store;

pub use crate::infra::link::{ConnectSpec, TransportConfig};
pub use store::{ArcLink, EndpointRef, Link, LinkId, LinkInfo, LinkRegistry};

#[derive(Clone, Default)]
pub struct LinkService {
    links: Arc<LinkRegistry>,
}

impl LinkService {
    pub fn new() -> Self {
        Self::default()
    }

    pub async fn get(&self, id: LinkId) -> Option<ArcLink> {
        self.links.get(id).await
    }

    pub async fn list(&self) -> Vec<ArcLink> {
        self.links.list().await
    }

    pub async fn insert(&self, link: Link) -> ArcLink {
        self.links.insert(link).await
    }

    pub async fn remove(&self, id: LinkId) -> Option<ArcLink> {
        self.links.remove(id).await
    }
}
