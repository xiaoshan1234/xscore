//! Link lifecycle and transport ownership.

use std::sync::Arc;

pub mod live;
pub mod registry;

pub use crate::domain::model::{ConnectSpec, EndpointRef, LinkInfo, TransportConfig};
pub use live::{ArcLink, Link};
pub use registry::LinkRegistry;

#[derive(Clone, Default)]
pub struct LinkManager {
    pub links: Arc<LinkRegistry>,
}

impl LinkManager {
    pub fn new() -> Self {
        Self::default()
    }
}
