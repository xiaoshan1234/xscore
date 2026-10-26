//! The UI model is currently headless: it owns Window/Pane rows and consumes
//! events. It never reads or writes a Link.

pub mod event;
pub mod pane;
pub mod pane_registry;
pub mod window;
pub mod window_registry;

pub use event::UiEvent;
pub use pane::{PaneId, PaneInfo};
pub use pane_registry::PaneRegistry;
pub use window::{Window, WindowBackend, WindowId, WindowInfo};
pub use window_registry::WindowRegistry;
