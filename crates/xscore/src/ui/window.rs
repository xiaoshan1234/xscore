// ui/window.rs (v0.3.0 — pure UI layout container, ADR 0007 v2)
//
// Window is a pure UI layout container. No `link_id` field — windows
// are independent entities that may exist with zero panes and may be
// orphan (no link relationship).
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;

use crate::service::session_service::SessionId;
use crate::ui::pane::PaneId;

uuid_id!(WindowId);

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WindowInfo {
    pub id: WindowId,
    pub session_id: SessionId,
    pub name: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub cols: u16,
    pub rows: u16,
    #[serde(default)]
    pub backend: WindowBackend,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum WindowBackend {
    #[default]
    Native,
    Tmux {
        handle: String,
    },
}

impl WindowBackend {
    pub fn tmux_handle(&self) -> Option<&str> {
        match self {
            WindowBackend::Native => None,
            WindowBackend::Tmux { handle } => Some(handle),
        }
    }

    pub fn is_tmux(&self) -> bool {
        self.tmux_handle().is_some()
    }
}

#[derive(Debug)]
pub struct Window {
    pub info: WindowInfo,
    pub panes: Mutex<Vec<PaneId>>,
}

impl Window {
    pub fn new(info: WindowInfo) -> Self {
        Self {
            info,
            panes: Mutex::new(Vec::new()),
        }
    }

    pub async fn add_pane(&self, pane_id: PaneId) {
        self.panes.lock().await.push(pane_id);
    }

    pub async fn remove_pane(&self, pane_id: PaneId) {
        self.panes.lock().await.retain(|p| *p != pane_id);
    }
}
