//! Raw backend: one or more Links, each converted to an independent Buffer.

use std::collections::HashMap;

use crate::domain::model::{BufferInfo, BufferOrigin};
use crate::domain::{BufferId, LinkId, SessionId};

#[derive(Clone)]
pub struct RawSessionManager {
    pub(super) session_id: SessionId,
    pub(super) links: Vec<LinkId>,
    pub(super) buffers: HashMap<LinkId, BufferId>,
}

impl RawSessionManager {
    pub(super) fn from_buffers(
        session_id: SessionId,
        links: Vec<LinkId>,
        buffers: &[&BufferInfo],
    ) -> Self {
        let mut by_link = HashMap::new();
        for buffer in buffers {
            if let BufferOrigin::Link { link_id } = buffer.origin {
                by_link.insert(link_id, buffer.id);
            }
        }
        Self {
            session_id,
            links,
            buffers: by_link,
        }
    }

    pub fn bind(&mut self, link: LinkId, buffer: BufferId) {
        if !self.links.contains(&link) {
            self.links.push(link);
        }
        self.buffers.insert(link, buffer);
    }

    pub fn unbind_link(&mut self, link: LinkId) {
        self.links.retain(|candidate| *candidate != link);
        self.buffers.remove(&link);
    }

    pub fn buffer_for_link(&self, link: LinkId) -> Option<BufferId> {
        self.buffers.get(&link).copied()
    }

    pub fn route(&self, link: LinkId, bytes: &[u8]) -> Option<(BufferId, Vec<u8>)> {
        self.buffer_for_link(link)
            .map(|buffer| (buffer, bytes.to_vec()))
    }
}
