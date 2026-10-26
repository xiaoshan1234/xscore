//! Session orchestration and the Raw/Tmux backend conversion layer.

pub mod manager;
pub mod raw_session;
pub mod registry;
pub mod runtime;
pub mod session;
pub mod tmux_session;

pub use crate::domain::model::{SessionBackend, SessionInfo, TmuxHostConfig, TmuxSocket};
pub use manager::{LinkAddResult, SessionManager, TmuxCreateOptions};
pub use registry::SessionRegistry;
pub use runtime::SessionManager as BackendSessionManager;
pub use session::Session;
pub use tmux_session::TmuxEventAction;
