// core/pane.rs
use super::id::{PaneId, WindowId};
use crate::model::PaneInfo;

pub struct Pane {
    pub info: PaneInfo,
}

impl Pane {
    pub fn id(&self) -> PaneId { self.info.id }
    /// Renamed from `link_id()` to match `PaneInfo.window_id` rename
    /// (reconciliation rule 3 in task-06 partition directive).
    pub fn window_id(&self) -> WindowId { self.info.window_id }
}
