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
use crate::core::{LinkAddress, LinkId, PaneId, SessionId, WindowId};

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
    /// v0.6.0 (ADR 0010 §1): the workspace this entity belongs to. A link is
    /// one connection *inside* a session — the session, not the link, says
    /// whether tmux is involved.
    pub session_id: SessionId,
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
    /// v0.6.0 (ADR 0010 §1): the workspace this entity belongs to — see
    /// `LinkInfo::session_id`.
    pub session_id: SessionId,
    pub name: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// Layout shape the window presents to the user (ADR 0007 v2 — Q2-adjacent).
    pub layout: WindowLayout,
    /// Total window size in cells. The window's panes fit inside these
    /// dimensions (the pane reports its own geometry separately).
    pub cols: u16,
    pub rows: u16,
    /// v0.4.0 (ADR 0008): the tmux window handle, e.g. `"@0"`. `None`
    /// for virtual (purely xscore-side) windows. This is the identity
    /// anchor that lets a WindowId be mapped back to tmux after a
    /// restart — never use the tmux window *index* for identity (it
    /// shifts when windows are added or removed).
    #[serde(default)]
    pub tmux_window_handle: Option<String>,
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
    pub id: PaneId,
    /// v0.6.0 (ADR 0010 §1): the workspace this entity belongs to — see
    /// `LinkInfo::session_id`. A pane's session is fixed at creation; the
    /// manager refuses to bind it to a link from another session.
    pub session_id: SessionId,
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
    /// v0.4.0 (ADR 0008): the tmux pane handle, e.g. `"%3"`. `None` for
    /// direct panes (buffered by `DirectBridge`'s ring buffer). When
    /// `Some`, this pane is OWNED by tmux: it cannot be detached, and
    /// it dies with the tmux session.
    #[serde(default)]
    pub sink_handle: Option<String>,
}

// ===== v0.6.0 — the Session tier (ADR 0010 §1) =====

/// The workspace: the entity that owns windows, panes and links.
///
/// v0.6.0 (ADR 0010): "does this go through tmux?" moved here from the link.
/// A session holds N windows, N panes and its links; the children carry
/// `session_id`, so there is no child list to keep in sync.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SessionInfo {
    pub id: SessionId,
    pub name: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub backend: SessionBackend,
}

/// Who keeps this workspace alive.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SessionBackend {
    /// Links are byte pipes; the daemon owns them. Dies when xscore forgets
    /// the session.
    Native,
    /// This session IS a tmux session: `session` is the user-visible tmux
    /// session name on `host`, reached through the server at `socket`.
    Tmux {
        host: TmuxHostConfig,
        socket: TmuxSocket,
        session: String,
    },
}

/// Where the tmux server runs (ADR 0010 §6).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TmuxHostConfig {
    Local,
    Ssh { destination: String, args: Vec<String> },
}

/// Which tmux server on that host (ADR 0010 §6).
///
/// A named socket resolves *on the host*, so a name is not a location — that
/// is why this is not `Option<PathBuf>`. On disk: `{"kind":"default"}`,
/// `{"kind":"named","name":"x"}`, `{"kind":"path","path":"/tmp/s"}`.
///
/// Serialized by hand (v0.6.0): `Named(String)` is a newtype variant wrapping a
/// scalar, and serde's internally-tagged representation cannot inline a bare
/// string — deriving here compiles and then fails on the first `to_string`
/// ("cannot serialize tagged newtype variant ... containing a string"). The
/// wire enum below gives the payload a field of its own instead of changing the
/// API shape to `Named { name: String }`.
#[derive(Debug, Clone, PartialEq)]
pub enum TmuxSocket {
    Default,
    Named(String),
    Path(String),
}

#[derive(Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum TmuxSocketWire<'a> {
    Default,
    Named { name: &'a str },
    Path { path: &'a str },
}

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum TmuxSocketRepr {
    Default,
    Named { name: String },
    Path { path: String },
}

impl Serialize for TmuxSocket {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let wire = match self {
            TmuxSocket::Default => TmuxSocketWire::Default,
            TmuxSocket::Named(name) => TmuxSocketWire::Named { name },
            TmuxSocket::Path(path) => TmuxSocketWire::Path { path },
        };
        wire.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for TmuxSocket {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Ok(match TmuxSocketRepr::deserialize(deserializer)? {
            TmuxSocketRepr::Default => TmuxSocket::Default,
            TmuxSocketRepr::Named { name } => TmuxSocket::Named(name),
            TmuxSocketRepr::Path { path } => TmuxSocket::Path(path),
        })
    }
}

impl SessionInfo {
    pub fn new_native(name: impl Into<String>) -> Self {
        Self {
            id: SessionId::new(),
            name: name.into(),
            created_at: chrono::Utc::now(),
            backend: SessionBackend::Native,
        }
    }

    pub fn new_tmux(
        name: impl Into<String>,
        host: TmuxHostConfig,
        socket: TmuxSocket,
        session: impl Into<String>,
    ) -> Self {
        Self {
            id: SessionId::new(),
            name: name.into(),
            created_at: chrono::Utc::now(),
            backend: SessionBackend::Tmux { host, socket, session: session.into() },
        }
    }

    pub fn is_tmux(&self) -> bool {
        matches!(self.backend, SessionBackend::Tmux { .. })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_backend_is_tagged_object() {
        // Internally-tagged unit variants must become `{"kind":"native"}` —
        // a bare `"native"` would fail to deserialize back.
        let s = serde_json::to_string(&SessionBackend::Native).unwrap();
        assert_eq!(s, r#"{"kind":"native"}"#);
        let back: SessionBackend = serde_json::from_str(&s).unwrap();
        assert_eq!(back, SessionBackend::Native);
    }

    #[test]
    fn tmux_socket_named_roundtrips() {
        let socket = TmuxSocket::Named("x".into());
        let s = serde_json::to_string(&socket).unwrap();
        assert!(s.contains("\"named\""), "tag missing: {s}");
        assert!(s.contains("\"x\""), "payload missing: {s}");
        let back: TmuxSocket = serde_json::from_str(&s).unwrap();
        assert_eq!(back, socket);
    }

    #[test]
    fn tmux_socket_default_and_path_roundtrip() {
        for socket in [TmuxSocket::Default, TmuxSocket::Path("/tmp/s".into())] {
            let s = serde_json::to_string(&socket).unwrap();
            let back: TmuxSocket = serde_json::from_str(&s).unwrap();
            assert_eq!(back, socket);
        }
    }

    #[test]
    fn session_info_roundtrips_with_its_children() {
        let info = SessionInfo::new_tmux(
            "work",
            TmuxHostConfig::Ssh { destination: "devbox".into(), args: vec![] },
            TmuxSocket::Default,
            "tmux-work",
        );
        assert!(info.is_tmux());
        let s = serde_json::to_string(&info).unwrap();
        let back: SessionInfo = serde_json::from_str(&s).unwrap();
        assert_eq!(back, info);
        assert!(!SessionInfo::new_native("plain").is_tmux());
    }
}