use crate::service::buffer_service::BufferInfo;
use crate::service::link_service::LinkInfo;
use crate::service::session_service::SessionInfo;
use crate::ui::{PaneId, PaneInfo, WindowInfo};

pub const SCHEMA_VERSION: u32 = 3;

/// v0.6.0 flat snapshot: the three entity lists plus the sessions that own
/// them. `sessions` comes first on disk so a human reading the file learns the
/// workspace before its children.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct StateSnapshot {
    /// Must equal `SCHEMA_VERSION`. No serde default on purpose.
    pub schema_version: u32,
    pub sessions: Vec<SessionInfo>,
    pub links: Vec<LinkInfo>,
    #[serde(default)]
    pub buffers: Vec<BufferInfo>,
    pub windows: Vec<WindowSnapshot>,
    pub panes: Vec<PaneInfo>,
}

impl Default for StateSnapshot {
    fn default() -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            sessions: Vec::new(),
            links: Vec::new(),
            buffers: Vec::new(),
            windows: Vec::new(),
            panes: Vec::new(),
        }
    }
}

/// One window's worth of persisted state. Flattened `WindowInfo` + the
/// list of pane ids that currently live in this window. (Pane ids only —
/// the pane's full `PaneInfo` lives in `StateSnapshot.panes`.)
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct WindowSnapshot {
    #[serde(flatten)]
    pub info: WindowInfo,
    pub panes: Vec<PaneId>,
}
