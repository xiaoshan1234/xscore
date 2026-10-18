// model/session.rs
use serde::{Deserialize, Serialize};
use crate::core::{LinkAddress, LinkId, PaneId, WindowId};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "transport", rename_all = "snake_case")]
pub enum TransportConfig {
    Shell { command: String, #[serde(default)] args: Vec<String>, #[serde(default)] cwd: Option<String> },
    Serial { path: String, baud: u32, #[serde(default = "default_data_bits")] data_bits: u8,
             #[serde(default = "default_parity")] parity: String, #[serde(default = "default_stop_bits")] stop_bits: u8 },
    Ssh { host: String, #[serde(default = "default_ssh_port")] port: u16, user: String,
          #[serde(default)] identity: Option<String>, #[serde(default)] proxy: Option<String> },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "sink", rename_all = "snake_case")]
pub enum SinkConfig {
    Tmux {
        #[serde(default = "default_tmux_mode")]
        mode: String,
        #[serde(default)]
        session_name: Option<String>,
    },
}

fn default_ssh_port() -> u16 { 22 }
fn default_data_bits() -> u8 { 8 }
fn default_parity() -> String { "n".into() }
fn default_stop_bits() -> u8 { 1 }
fn default_tmux_mode() -> String { "auto".into() }

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ConnectSpec {
    #[serde(default = "default_name")]
    pub name: String,
    #[serde(flatten)]
    pub transport: TransportConfig,
    pub sink: SinkConfig,
}

fn default_name() -> String { uuid::Uuid::now_v7().to_string() }

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct LinkInfo {
    pub id: LinkId,
    pub name: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub address: LinkAddress,
    pub transport: TransportConfig,
    pub sink: SinkConfig,
}

impl LinkInfo {
    pub fn transport_str(&self) -> &'static str { self.address.kind() }
    pub fn sink_str(&self) -> &'static str {
        match &self.sink {
            SinkConfig::Tmux { .. } => "tmux",
        }
    }
}

/// ADR 0006: a window is a tmux window inside the link's session. The link
/// owns the session, windows own panes (which are tmux splits).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WindowInfo {
    pub id: WindowId,
    pub link_id: LinkId,
    pub name: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// Index of this window inside its link's tmux session (tmux assigns
    /// these incrementally; used to address the window via tmux wire
    /// commands like `tmux -L xscore kill-window -t <session>:<idx>`).
    pub tmux_window_index: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PaneInfo {
    pub id: PaneId,
    pub window_id: WindowId,    // was: link_id
    pub name: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub sink_handle: String,    // tmux pane_id like "%7"
    pub pane_index: u32,        // 0-based, split index inside the window
}
