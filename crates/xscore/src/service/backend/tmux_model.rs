//! tmux control-plane structure exchanged with a DaemonPort implementation.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TmuxWindowRow {
    pub handle: String,
    pub name: String,
    pub cols: u16,
    pub rows: u16,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TmuxPaneRow {
    pub handle: String,
    pub cols: u16,
    pub rows: u16,
    pub x: u16,
    pub y: u16,
    pub window_handle: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct TmuxStructure {
    pub windows: Vec<TmuxWindowRow>,
    pub panes: Vec<TmuxPaneRow>,
}
