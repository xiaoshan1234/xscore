//! Reusable terminal emulator server state, command handling, and daemon runtime.

/// Persistent application state and pane/group/layout registries.
pub mod app;
/// Server-side command handlers and dispatch.
pub mod commands;
/// Layout tree and pane geometry.
pub mod layout;
/// Pane output logger registry.
pub mod logger;
/// File and stderr logging setup.
pub mod logging;
/// `PaneHandle` and terminal-core bindings.
pub mod pane;
/// Text rendering for the CLI protocol.
pub mod render;
/// Loopback daemon runtime.
pub mod server;
/// tmux event processing and session reconciliation.
pub mod tmux;
/// Window and group identifiers.
pub mod window_group;
