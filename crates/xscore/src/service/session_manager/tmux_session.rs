//! Tmux backend: one control Link, `%N` output demultiplexed into Buffers.

use std::collections::HashMap;

use crate::domain::model::{BufferInfo, BufferOrigin};
use crate::domain::{BufferId, LinkId, SessionId};
use crate::infra::tmux::TmuxEvent;
use crate::ui::UiEvent;

/// What the top-level registry owner must do after a tmux SessionManager has
/// interpreted a control-mode event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TmuxEventAction {
    Ui(UiEvent),
    Exit(String),
    Ignore,
}

#[derive(Clone)]
pub struct TmuxSessionManager {
    pub(super) session_id: SessionId,
    pub(super) link_id: LinkId,
    pub(super) buffers: HashMap<String, BufferId>,
}

impl TmuxSessionManager {
    pub(super) fn from_buffers(
        session_id: SessionId,
        link_id: LinkId,
        buffers: &[&BufferInfo],
    ) -> Self {
        let mut by_handle = HashMap::new();
        for buffer in buffers {
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
        Self {
            session_id,
            link_id,
            buffers: by_handle,
        }
    }

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

    pub fn accept_event(&self, event: &TmuxEvent) -> TmuxEventAction {
        match event {
            TmuxEvent::WindowAdd { handle }
            | TmuxEvent::WindowRenamed { handle }
            | TmuxEvent::LayoutChange { handle, .. } => {
                TmuxEventAction::Ui(UiEvent::WindowChanged {
                    tmux_handle: handle.clone(),
                })
            }
            TmuxEvent::WindowClose { handle } => TmuxEventAction::Ui(UiEvent::WindowRemoved {
                tmux_handle: handle.clone(),
            }),
            TmuxEvent::Exit { reason } => TmuxEventAction::Exit(reason.clone()),
            TmuxEvent::SessionWindowChanged { .. }
            | TmuxEvent::SessionChanged { .. }
            | TmuxEvent::SessionsChanged => TmuxEventAction::Ignore,
        }
    }
}
