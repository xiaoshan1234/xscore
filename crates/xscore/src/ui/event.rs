//! Events emitted by backend services and consumed by the UI layer.

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UiEvent {
    WindowChanged { tmux_handle: String },
    WindowRemoved { tmux_handle: String },
    PaneChanged { tmux_handle: String },
    PaneRemoved { tmux_handle: String },
}
