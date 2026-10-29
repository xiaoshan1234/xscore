//! Raw backend route from each Link to its independent Buffer.

use std::collections::{HashMap, HashSet};

use crate::service::buffer_service::{BufferId, BufferInfo, BufferOrigin};
use crate::service::link_service::LinkId;

#[derive(Clone, Default)]
pub struct RawBackend {
    buffers: HashMap<LinkId, BufferId>,
}

impl RawBackend {
    pub(super) fn from_buffers(links: &[LinkId], buffers: &[&BufferInfo]) -> Self {
        let links: HashSet<LinkId> = links.iter().copied().collect();
        let buffers = buffers
            .iter()
            .filter_map(|buffer| match buffer.origin {
                BufferOrigin::Link { link_id } if links.contains(&link_id) => {
                    Some((link_id, buffer.id))
                }
                _ => None,
            })
            .collect();
        Self { buffers }
    }

    pub fn buffer_for_link(&self, link: LinkId) -> Option<BufferId> {
        self.buffers.get(&link).copied()
    }
}
