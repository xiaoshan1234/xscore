// model/session.rs
//
// v0.3.0 — 3-entity decoupled model (ADR 0007 rewrite).
//
// Three independent entities:
//   - Link   = transport + sink + the link's connection state (no windows/panes owned)
//   - Pane   = core is a buffer (here represented as `bridge: Option<Arc<dyn PaneBridge>>`)
//              + UI info (geometry cols/rows/x/y + which link/window it's attached to)
//   - Window = pure UI layout container (panes list + layout + total cols/rows)
//
// Associations are dynamic via Option IDs on Pane.info.link_id / Pane.info.window_id.
// Windows and panes are orphan-creatable (no link required). Links can be created
// without any window/pane (the default in v0.3.0 — see ADR 0007 v2).
//
// A direct link tracks ONE active pane at a time (Q1 — user-confirmed 2026-10-19):
// when a new pane is attached, the previously-active pane is auto-detached (its
// `link_id` becomes `None`, its buffer survives). A tmux-backed link has no such
// restriction; multiple panes can be attached simultaneously (Q1 corollary).

use serde::{Deserialize, Serialize};
use crate::core::{LinkAddress, LinkId, WindowId};

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
    /// tmux overhead.
    Raw,
    /// Wrap the link in a tmux session (per ADR 0003, the shared
    /// `tmux -L xscore` server). `mode` controls auto/local/remote;
    /// `session_name` is an optional override for the auto-generated
    /// `xscore-<link_id>` name.
    ///
    /// Opt in via the URL query `?tmux=on` (defaults to mode=auto)
    /// or `?tmux=auto|local|remote` (explicit mode).
    ///
    /// v0.3.0 semantics: a tmux-backed link is multi-pane capable —
    /// multiple panes may be attached simultaneously (Q1 corollary).
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

/// v0.3.0: Window is a pure UI layout container (ADR 0007 rewrite).
/// It does NOT know its parent link — windows are independent entities
/// that may be empty of panes and may exist without any link.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WindowInfo {
    pub id: WindowId,
    pub name: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// Layout shape the window presents to the user (ADR 0007 v2 — Q2-adjacent).
    pub layout: WindowLayout,
    /// Total window size in cells. The window's panes fit inside these
    /// dimensions (the pane reports its own geometry separately).
    pub cols: u16,
    pub rows: u16,
}

/// Window layout shape (ADR 0007 v2). The pane geometry in `PaneInfo`
/// is the source of truth for where each pane actually appears; the
/// window's `layout` is a hint for how to arrange them. `Single` is the
/// default for new windows.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum WindowLayout {
    /// Single pane fills the window (the default).
    #[serde(rename = "single")]
    Single,
    /// Tab bar at the top; panes are full-window and switchable.
    #[serde(rename = "tabs")]
    Tabs,
    /// All panes stacked left-to-right (horizontal split).
    #[serde(rename = "split_h")]
    SplitHorizontal,
    /// All panes stacked top-to-bottom (vertical split).
    #[serde(rename = "split_v")]
    SplitVertical,
    /// Uniform NxM grid of panes.
    #[serde(rename = "grid")]
    Grid { cols: u16, rows: u16 },
}

impl Default for WindowLayout {
    fn default() -> Self { WindowLayout::Single }
}

/// v0.3.0: Pane is the buffer-holder + UI geometry (ADR 0007 rewrite).
///
/// The pane's *buffer* is `Pane.bridge: Option<Arc<dyn PaneBridge>>` —
/// `None` means the pane is orphan (no link attached); `Some(bridge)`
/// means it has a live I/O channel (DirectBridge for raw links,
/// TmuxBridge for tmux-backed links). The pane keeps its buffer
/// content even when detached (it's the bridge that closes; the pane
/// itself stays).
///
/// The pane's *UI info* is the geometry fields (cols/rows/x/y) +
/// which window it lives in + which link it's currently attached to.
/// Both `link_id` and `window_id` are `Option<...>` — the pane is
/// orphan-creatable.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PaneInfo {
    pub id: crate::core::PaneId,
    pub name: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
    // Geometry (Q2 — pane's UI info is cols/rows/x/y).
    pub cols: u16,
    pub rows: u16,
    pub x: u16,
    pub y: u16,
    /// Currently attached link (None = orphan pane). Set by
    /// `xscore pane attach --pane <pid> --link <lid>`; cleared by
    /// `xscore pane detach` or by `del_link` / `del_pane`.
    pub link_id: Option<LinkId>,
    /// Currently parent window (None = orphan pane, hidden). Set by
    /// `xscore pane add-to-window --pane <pid> --window <wid>`; cleared
    /// by `xscore pane remove-from-window`.
    pub window_id: Option<WindowId>,
}