pub mod address;
pub mod error;
pub mod id;
pub mod link;
pub mod link_registry;
pub mod manager;
pub mod pane;
pub mod pane_registry;
pub mod window;
pub mod window_registry;

pub use address::LinkAddress;
pub use error::{Error, Result};
pub use id::{LinkId, PaneId, WindowId};
