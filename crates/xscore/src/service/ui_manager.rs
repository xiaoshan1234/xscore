//! Coordinates the UI registries without owning bytes or connections.

use std::sync::Arc;

use crate::ui::pane_registry::PaneRegistry;
use crate::ui::window_registry::WindowRegistry;

#[derive(Clone, Default)]
pub struct UiManager {
    pub windows: Arc<WindowRegistry>,
    pub panes: Arc<PaneRegistry>,
}

impl UiManager {
    pub fn new() -> Self {
        Self::default()
    }
}
