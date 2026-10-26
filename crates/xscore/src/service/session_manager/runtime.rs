//! Dispatch between the Raw and Tmux SessionManager implementations.

use crate::domain::model::{BufferInfo, SessionBackend, SessionInfo};
use crate::domain::{BufferId, Error, LinkId, Result, SessionId};
use crate::infra::tmux::TmuxEvent;

use super::raw_session::RawSessionManager;
use super::tmux_session::{TmuxEventAction, TmuxSessionManager};

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
                Ok(Self::Raw(RawSessionManager::from_buffers(
                    session.id,
                    links.to_vec(),
                    &own_buffers,
                )))
            }
            SessionBackend::Tmux { .. } => {
                if links.len() != 1 {
                    return Err(Error::Invalid(
                        "a tmux session requires exactly one control link".into(),
                    ));
                }
                Ok(Self::Tmux(TmuxSessionManager::from_buffers(
                    session.id,
                    links[0],
                    &own_buffers,
                )))
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

    pub fn route_output(
        &self,
        link: LinkId,
        handle: &str,
        bytes: &[u8],
    ) -> Option<(BufferId, Vec<u8>)> {
        match self {
            Self::Raw(manager) => manager.route(link, bytes),
            Self::Tmux(manager) => manager.route_output(link, handle, bytes),
        }
    }

    /// Translate a tmux protocol event into a backend-level action.
    pub fn accept_tmux_event(&self, event: &TmuxEvent) -> Option<TmuxEventAction> {
        match self {
            Self::Raw(_) => None,
            Self::Tmux(manager) => Some(manager.accept_event(event)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::model::{BufferOrigin, TmuxHostConfig, TmuxSocket};
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
        let info =
            SessionInfo::new_tmux("tmux", TmuxHostConfig::Local, TmuxSocket::Default, "tmux");
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

        let tmux =
            SessionInfo::new_tmux("tmux", TmuxHostConfig::Local, TmuxSocket::Default, "tmux");
        let tmux_link = LinkId::new();
        let tmux_manager =
            SessionManager::from_parts(&tmux, &[tmux_link], &[]).expect("tmux manager");
        assert_eq!(
            tmux_manager.accept_tmux_event(&TmuxEvent::LayoutChange {
                handle: "@7".into(),
                layout: "layout".into(),
            }),
            Some(TmuxEventAction::Ui(crate::ui::UiEvent::WindowChanged {
                tmux_handle: "@7".into()
            }))
        );
        assert_eq!(
            tmux_manager.accept_tmux_event(&TmuxEvent::WindowClose {
                handle: "@8".into(),
            }),
            Some(TmuxEventAction::Ui(crate::ui::UiEvent::WindowRemoved {
                tmux_handle: "@8".into()
            }))
        );
    }
}
