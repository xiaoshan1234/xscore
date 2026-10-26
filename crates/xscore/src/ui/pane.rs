// ui/pane.rs — pure Pane UI metadata
//
// `PaneInfo` contains only viewport geometry, window placement and an optional
// Buffer reference. Bytes, source Links and tmux handles belong to BufferInfo
// and the backend SessionService.
//
// The registry and manager keep UI metadata separate from runtime runtimes.

use serde::{Deserialize, Serialize};

use crate::service::buffer_service::BufferId;
use crate::service::session_service::SessionId;
use crate::ui::window::WindowId;

uuid_id!(PaneId);

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PaneInfo {
    pub id: PaneId,
    pub session_id: SessionId,
    pub name: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub cols: u16,
    pub rows: u16,
    pub x: u16,
    pub y: u16,
    #[serde(default)]
    pub buffer_id: Option<BufferId>,
    pub window_id: Option<WindowId>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pane_id_roundtrip_via_string() {
        let id = PaneId::new();
        assert_eq!(PaneId::parse(&id.to_string()).unwrap(), id);
    }
}
