//! Shared domain kernel.
//!
//! Domain modules may be used by UI, service and infrastructure code. They do
//! not depend on any of those outer layers.

pub mod address;
pub mod buffer;
pub mod error;
pub mod id;
pub mod model;

pub use address::LinkAddress;
pub use buffer::{RingBuffer, DIRECT_BUFFER_CAPACITY};
pub use error::{Error, Result};
pub use id::{BufferId, LinkId, PaneId, SessionId, WindowId};
