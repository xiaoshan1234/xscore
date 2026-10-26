//! Session use cases and the service facade over backend, link, buffer and UI.

pub mod model;
pub mod service;
pub mod store;

mod data_methods;
mod link_methods;
mod persistence_methods;
mod tmux_methods;
mod tmux_reconcile_methods;
mod ui_methods;

pub use crate::infra::tmux::{TmuxHostConfig, TmuxSocket};
pub use model::{SessionBackend, SessionId, SessionInfo};
pub use service::{LinkAddResult, SessionService, TmuxCreateOptions};
pub use store::{Session, SessionRegistry};
