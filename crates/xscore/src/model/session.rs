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
    /// Direct transport I/O. No tmux server involvement — the
    /// shell/serial/ssh transport is the I/O source directly. Use
    /// for simple pipe-like scenarios or when you don't want the
    /// tmux overhead (and the `xscore window` / `xscore pane`
    /// subcommands don't apply to a raw link).
    Raw,
    /// Wrap the link in a tmux session (per ADR 0003, the shared
    /// `tmux -L xscore` server). `mode` controls auto/local/remote;
    /// `session_name` is an optional override for the auto-generated
    /// `xscore-<link_id>` name.
    ///
    /// Opt in via the URL query `?tmux=on` (defaults to mode=auto)
    /// or `?tmux=auto|local|remote` (explicit mode).
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
    /// Persisted address — for the "save URL as template" workflow, the
    /// user types `xscore link config add <name> <url>`; the address is
    /// parsed from the URL and stored here so `link add -c <name>` can
    /// recover the routing address without re-deriving it from the
    /// transport. (ADR 0005 retired `--link=existing`, but templates are
    /// a different concept — they let the user save a `name` and reuse
    /// the routing address on subsequent `link add` calls.)
    pub address: LinkAddress,
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
            SinkConfig::Raw => "raw",
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
