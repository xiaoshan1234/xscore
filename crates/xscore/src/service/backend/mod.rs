//! Dispatch between the Raw and Tmux BackendRuntime implementations.

use crate::infra::tmux::TmuxEvent;
use crate::service::buffer_service::BufferId;
use crate::service::buffer_service::BufferInfo;
use crate::service::link_service::LinkId;
use crate::service::session_service::{SessionBackend, SessionInfo};
use crate::{Error, Result};

pub mod daemon_port;
pub mod raw;
pub mod tmux;
pub mod tmux_model;
pub(crate) mod tmux_runtime;

use raw::RawBackend;
use tmux::TmuxBackend;

pub use tmux::BackendEvent;
pub(crate) use tmux_runtime::TmuxRuntime;

#[derive(Clone)]
pub enum BackendRuntime {
    Raw(RawBackend),
    Tmux(TmuxBackend),
}

impl BackendRuntime {
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
                Ok(Self::Raw(RawBackend::from_buffers(links, &own_buffers)))
            }
            SessionBackend::Tmux { .. } => {
                if links.len() != 1 {
                    return Err(Error::Invalid(
                        "a tmux session requires exactly one control link".into(),
                    ));
                }
                Ok(Self::Tmux(TmuxBackend))
            }
        }
    }

    pub fn buffer_for_link(&self, link: LinkId) -> Option<BufferId> {
        match self {
            Self::Raw(manager) => manager.buffer_for_link(link),
            Self::Tmux(_) => None,
        }
    }

    /// Translate a tmux protocol event into a backend-level action.
    pub fn accept_tmux_event(&self, event: &TmuxEvent) -> Option<BackendEvent> {
        match self {
            Self::Raw(_) => None,
            Self::Tmux(manager) => Some(manager.accept_event(event)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::infra::tmux::{TmuxHostConfig, TmuxSocket};
    use crate::service::buffer_service::BufferOrigin;
    use crate::service::session_service::SessionId;
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
        let manager = BackendRuntime::from_parts(&info, &[l1, l2], &buffers).unwrap();
        assert_eq!(manager.buffer_for_link(l1), Some(b1));
        assert_eq!(manager.buffer_for_link(l2), Some(b2));
    }

    #[test]
    fn tmux_backend_requires_exactly_one_control_link() {
        let info =
            SessionInfo::new_tmux("tmux", TmuxHostConfig::Local, TmuxSocket::Default, "tmux");
        let link = LinkId::new();
        BackendRuntime::from_parts(&info, &[link], &[]).unwrap();
        assert!(BackendRuntime::from_parts(&info, &[link, LinkId::new()], &[]).is_err());
    }

    #[test]
    fn only_a_tmux_manager_translates_control_events() {
        let raw = SessionInfo::new_native("raw");
        let raw_link = LinkId::new();
        let raw_manager = BackendRuntime::from_parts(&raw, &[raw_link], &[]).expect("raw manager");
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
            BackendRuntime::from_parts(&tmux, &[tmux_link], &[]).expect("tmux manager");
        assert_eq!(
            tmux_manager.accept_tmux_event(&TmuxEvent::LayoutChange {
                handle: "@7".into(),
                layout: "layout".into(),
            }),
            Some(BackendEvent::Ui(crate::ui::UiEvent::WindowChanged {
                tmux_handle: "@7".into()
            }))
        );
        assert_eq!(
            tmux_manager.accept_tmux_event(&TmuxEvent::WindowClose {
                handle: "@8".into(),
            }),
            Some(BackendEvent::Ui(crate::ui::UiEvent::WindowRemoved {
                tmux_handle: "@8".into()
            }))
        );
    }
}
