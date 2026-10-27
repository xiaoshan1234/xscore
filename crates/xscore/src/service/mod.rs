//! Application services.
//!
//! Backends interpret connections, BufferService owns byte runtimes,
//! LinkService owns connections, UiProjection owns structure projection and
//! SessionService coordinates use cases.

pub mod backend;
pub mod buffer_service;
pub mod link_service;
pub mod persistence;
pub mod session_service;
pub mod ui_projection;
