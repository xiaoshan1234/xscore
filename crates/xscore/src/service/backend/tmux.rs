//! Tmux backend event translation.

use crate::infra::tmux::TmuxEvent;
use crate::ui::UiEvent;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BackendEvent {
    Ui(UiEvent),
    Exit(String),
    Ignore,
}

#[derive(Clone, Copy, Default)]
pub struct TmuxBackend;

impl TmuxBackend {
    pub fn accept_event(self, event: &TmuxEvent) -> BackendEvent {
        match event {
            TmuxEvent::WindowAdd { handle }
            | TmuxEvent::WindowRenamed { handle }
            | TmuxEvent::LayoutChange { handle, .. } => BackendEvent::Ui(UiEvent::WindowChanged {
                tmux_handle: handle.clone(),
            }),
            TmuxEvent::WindowClose { handle } => BackendEvent::Ui(UiEvent::WindowRemoved {
                tmux_handle: handle.clone(),
            }),
            TmuxEvent::Exit { reason } => BackendEvent::Exit(reason.clone()),
            TmuxEvent::SessionWindowChanged { .. }
            | TmuxEvent::SessionChanged { .. }
            | TmuxEvent::SessionsChanged => BackendEvent::Ignore,
        }
    }
}
