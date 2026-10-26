//! Backend-specific conversion from link bytes to independent buffers.
//!
//! This is the layer between Links and Buffers. Panes are not involved here:
//! they are UI viewports that may point at any buffer owned by the session.

use std::collections::HashMap;

use super::{BufferId, Error, LinkId, Result, SessionId};
use crate::model::{BufferInfo, BufferOrigin, SessionBackend, SessionInfo};
use crate::tmux::TmuxEvent;

/// What the top-level registry owner must do after a tmux SessionManager has
/// interpreted a control-mode event. The SessionManager owns backend meaning;
/// the top-level manager owns the Window/Pane registries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TmuxEventAction {
    RefreshWindow(String),
    RemoveWindow(String),
    Exit(String),
    Ignore,
}

#[derive(Clone)]
pub enum SessionManager {
    Raw(RawSessionManager),
    Tmux(TmuxSessionManager),
}

impl SessionManager {
    pub fn from_parts(
        session: &SessionInfo,
        links: &[LinkId],
        buffers: &[BufferInfo],
    ) -> Result<Self> {
        let own_buffers: Vec<&BufferInfo> = buffers
            .iter()
            .filter(|buffer| buffer.session_id == session.id)
            .collect();
        match session.backend {
            SessionBackend::Native => {
                if links.is_empty() {
                    return Err(Error::Invalid(
                        "a raw session requires at least one link".into(),
                    ));
                }
                let mut by_link = HashMap::new();
                for buffer in own_buffers {
                    if let BufferOrigin::Link { link_id } = buffer.origin {
                        by_link.insert(link_id, buffer.id);
                    }
                }
                Ok(Self::Raw(RawSessionManager {
                    session_id: session.id,
                    links: links.to_vec(),
                    buffers: by_link,
                }))
            }
            SessionBackend::Tmux { .. } => {
                if links.len() != 1 {
                    return Err(Error::Invalid(
                        "a tmux session requires exactly one control link".into(),
                    ));
                }
                let link_id = links[0];
                let mut by_handle = HashMap::new();
                for buffer in own_buffers {
                    if let BufferOrigin::Tmux {
                        link_id: origin_link,
                        pane_handle,
                    } = &buffer.origin
                    {
                        if *origin_link == link_id {
                            by_handle.insert(pane_handle.clone(), buffer.id);
                        }
                    }
                }
                Ok(Self::Tmux(TmuxSessionManager {
                    session_id: session.id,
                    link_id,
                    buffers: by_handle,
                }))
            }
        }
    }

    pub fn kind(&self) -> &'static str {
        match self {
            Self::Raw(_) => "raw",
            Self::Tmux(_) => "tmux",
        }
    }

    pub fn session_id(&self) -> SessionId {
        match self {
            Self::Raw(manager) => manager.session_id,
            Self::Tmux(manager) => manager.session_id,
        }
    }

    pub fn buffer_for_link(&self, link: LinkId) -> Option<BufferId> {
        match self {
            Self::Raw(manager) => manager.buffer_for_link(link),
            Self::Tmux(_) => None,
        }
    }

    pub fn buffer_for_tmux_handle(&self, handle: &str) -> Option<BufferId> {
        match self {
            Self::Raw(_) => None,
            Self::Tmux(manager) => manager.buffer_for_handle(handle),
        }
    }

    /// Translate a tmux protocol event into a backend-level action.
    ///
    /// Raw managers have no tmux control stream, so they return no action.
    pub fn accept_tmux_event(&self, event: &TmuxEvent) -> Option<TmuxEventAction> {
        match self {
            Self::Raw(_) => None,
            Self::Tmux(_) => Some(match event {
                TmuxEvent::WindowAdd { handle }
                | TmuxEvent::WindowRenamed { handle }
                | TmuxEvent::LayoutChange { handle, .. } => {
                    TmuxEventAction::RefreshWindow(handle.clone())
                }
                TmuxEvent::WindowClose { handle } => TmuxEventAction::RemoveWindow(handle.clone()),
                TmuxEvent::Exit { reason } => TmuxEventAction::Exit(reason.clone()),
                TmuxEvent::SessionWindowChanged { .. }
                | TmuxEvent::SessionChanged { .. }
                | TmuxEvent::SessionsChanged => TmuxEventAction::Ignore,
            }),
        }
    }
}

#[derive(Clone)]
pub struct RawSessionManager {
    session_id: SessionId,
    links: Vec<LinkId>,
    buffers: HashMap<LinkId, BufferId>,
}

impl RawSessionManager {
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

#[derive(Clone)]
pub struct TmuxSessionManager {
    session_id: SessionId,
    link_id: LinkId,
    buffers: HashMap<String, BufferId>,
}

impl TmuxSessionManager {
    pub fn control_link(&self) -> LinkId {
        self.link_id
    }

    pub fn bind_pane(&mut self, handle: String, buffer: BufferId) {
        self.buffers.insert(handle, buffer);
    }

    pub fn unbind_pane(&mut self, handle: &str) {
        self.buffers.remove(handle);
    }

    pub fn buffer_for_handle(&self, handle: &str) -> Option<BufferId> {
        self.buffers.get(handle).copied()
    }

    pub fn route_output(
        &self,
        link: LinkId,
        handle: &str,
        bytes: &[u8],
    ) -> Option<(BufferId, Vec<u8>)> {
        if link != self.link_id {
            return None;
        }
        self.buffer_for_handle(handle)
            .map(|buffer| (buffer, bytes.to_vec()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    fn buffer(id: BufferId, session: SessionId, origin: BufferOrigin) -> BufferInfo {
        BufferInfo {
            id,
            session_id: session,
            name: id.to_string(),
            created_at: Utc::now(),
            origin,
        }
    }

    #[test]
    fn raw_manager_routes_multiple_links_to_independent_buffers() {
        let l1 = LinkId::new();
        let l2 = LinkId::new();
        let b1 = BufferId::new();
        let b2 = BufferId::new();
        let info = SessionInfo::new_native("raw");
        let buffers = vec![
            buffer(b1, info.id, BufferOrigin::Link { link_id: l1 }),
            buffer(b2, info.id, BufferOrigin::Link { link_id: l2 }),
        ];
        let manager = SessionManager::from_parts(&info, &[l1, l2], &buffers).unwrap();
        assert_eq!(manager.session_id(), info.id);
        assert_eq!(manager.buffer_for_link(l1), Some(b1));
        assert_eq!(manager.buffer_for_link(l2), Some(b2));
        assert_eq!(manager.kind(), "raw");
    }

    #[test]
    fn tmux_manager_routes_one_link_by_pane_handle() {
        let info = SessionInfo::new_tmux(
            "tmux",
            crate::model::TmuxHostConfig::Local,
            crate::model::TmuxSocket::Default,
            "tmux",
        );
        let link = LinkId::new();
        let first = BufferId::new();
        let second = BufferId::new();
        let buffers = vec![
            buffer(
                first,
                info.id,
                BufferOrigin::Tmux {
                    link_id: link,
                    pane_handle: "%0".into(),
                },
            ),
            buffer(
                second,
                info.id,
                BufferOrigin::Tmux {
                    link_id: link,
                    pane_handle: "%1".into(),
                },
            ),
        ];
        let manager = SessionManager::from_parts(&info, &[link], &buffers).unwrap();
        assert_eq!(manager.kind(), "tmux");
        assert_eq!(manager.buffer_for_tmux_handle("%0"), Some(first));
        assert_eq!(manager.buffer_for_tmux_handle("%1"), Some(second));
        assert_eq!(manager.buffer_for_tmux_handle("%2"), None);
    }

    #[test]
    fn only_a_tmux_manager_translates_control_events() {
        let raw = SessionInfo::new_native("raw");
        let raw_link = LinkId::new();
        let raw_manager = SessionManager::from_parts(&raw, &[raw_link], &[]).expect("raw manager");
        assert_eq!(
            raw_manager.accept_tmux_event(&TmuxEvent::WindowAdd {
                handle: "@1".into(),
            }),
            None
        );

        let tmux = SessionInfo::new_tmux(
            "tmux",
            crate::model::TmuxHostConfig::Local,
            crate::model::TmuxSocket::Default,
            "tmux",
        );
        let tmux_link = LinkId::new();
        let tmux_manager =
            SessionManager::from_parts(&tmux, &[tmux_link], &[]).expect("tmux manager");
        assert_eq!(
            tmux_manager.accept_tmux_event(&TmuxEvent::LayoutChange {
                handle: "@7".into(),
                layout: "layout".into(),
            }),
            Some(TmuxEventAction::RefreshWindow("@7".into()))
        );
        assert_eq!(
            tmux_manager.accept_tmux_event(&TmuxEvent::WindowClose {
                handle: "@8".into(),
            }),
            Some(TmuxEventAction::RemoveWindow("@8".into()))
        );
    }
}
