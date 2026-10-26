// model/session.rs
//
// v0.6.0 (ADR 0010) — Session owns the backend.
//
//   - Session = the workspace, and the only place "does this go through tmux?"
//               is answered (`SessionBackend`).
//   - Link    = one connection inside a session: a byte pipe xscore owns, or
//               the session's single tmux control client (`EndpointRef`).
//   - Buffer  = independently managed bytes and history.
//   - SessionManager = backend conversion from Link input / tmux events to
//               Buffers (`Raw` or `Tmux`).
//   - Pane    = pure UI geometry and an optional Buffer reference.
//   - Window  = a tmux window (`WindowBackend::Tmux { handle }`) or a purely
//               xscore-side one (`Native`).
//
// A Pane references a Buffer through `buffer_id` and a Window through
// `window_id`; it carries no Link or tmux handle of its own. A raw
// SessionManager can route several Links to independent Buffers. A tmux
// SessionManager maps each `%N` on its one control Link to an independent
// Buffer.
//
// v0.6.0 PR4 removed the last traces of "tmux-ness on the link": the sink
// enum, the URL query that selected it, the layout enum and the window handle
// field are gone (see ADR 0010 §D7).

use crate::core::{BufferId, LinkAddress, LinkId, PaneId, SessionId, WindowId};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "transport", rename_all = "snake_case")]
pub enum TransportConfig {
    Shell {
        command: String,
        #[serde(default)]
        args: Vec<String>,
        #[serde(default)]
        cwd: Option<String>,
    },
    Serial {
        path: String,
        baud: u32,
        #[serde(default = "default_data_bits")]
        data_bits: u8,
        #[serde(default = "default_parity")]
        parity: String,
        #[serde(default = "default_stop_bits")]
        stop_bits: u8,
    },
    Ssh {
        host: String,
        #[serde(default = "default_ssh_port")]
        port: u16,
        user: String,
        #[serde(default)]
        identity: Option<String>,
        #[serde(default)]
        proxy: Option<String>,
    },
}

fn default_ssh_port() -> u16 {
    22
}
fn default_data_bits() -> u8 {
    8
}
fn default_parity() -> String {
    "n".into()
}
fn default_stop_bits() -> u8 {
    1
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ConnectSpec {
    #[serde(default = "default_name")]
    pub name: String,
    #[serde(flatten)]
    pub transport: TransportConfig,
    /// Persisted address — for the "save URL as template" workflow, the
    /// user types `xscore link config add <name> <url>`; the address is
    /// parsed from the URL and stored here so `link add -c <name>` can
    /// recover the routing address without re-deriving it from the
    /// transport. (ADR 0005 retired `--link=existing`, but templates are
    /// a different concept — they let the user save a `name` and reuse
    /// the routing address on subsequent `link add` calls.)
    pub address: LinkAddress,
}

fn default_name() -> String {
    uuid::Uuid::now_v7().to_string()
}

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
    /// What this connection carries (ADR 0010 §4). A `Pipe` link owns a
    /// transport and serves one pane; a `Control` link is the session's one
    /// tmux control client and carries nothing of its own — host, socket and
    /// session name live on `SessionBackend::Tmux`, so a link can never
    /// disagree with its session about which tmux server it is talking to.
    pub endpoint: EndpointRef,
}

/// Which side owns this link's stream (ADR 0010 §4).
///
/// `Pipe` is everything the old model called a "raw" link: one byte stream,
/// at most one reader. `Control` is the tmux control client of a session — one
/// connection to a server, demultiplexed by `%N`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum EndpointRef {
    Pipe(TransportConfig),
    Control,
}

impl LinkInfo {
    pub fn transport_str(&self) -> &'static str {
        self.address.kind()
    }

    /// True for the per-session control client (ADR 0010 §D2).
    pub fn is_control(&self) -> bool {
        matches!(self.endpoint, EndpointRef::Control)
    }
}

/// v0.6.0: a window belongs to a session and mirrors either nothing (native)
/// or one tmux window.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WindowInfo {
    pub id: WindowId,
    /// v0.6.0 (ADR 0010 §1): the workspace this entity belongs to — see
    /// `LinkInfo::session_id`.
    pub session_id: SessionId,
    pub name: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// Total window size in cells. The window's panes fit inside these
    /// dimensions (the pane reports its own geometry separately).
    pub cols: u16,
    pub rows: u16,
    /// Who owns this window's structure (ADR 0010 §D5).
    #[serde(default)]
    pub backend: WindowBackend,
}

/// Where a window's structure comes from.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum WindowBackend {
    /// A purely xscore-side window: no renderer exists yet, so it is a name and
    /// a geometry (arch.md §13).
    #[default]
    Native,
    /// A projection of a tmux window. `handle` is `@N` — the identity anchor,
    /// never the window *index*, which shifts when windows are added or
    /// removed (ADR 0008).
    Tmux { handle: String },
}

impl WindowBackend {
    /// The `@N` this window mirrors, if any.
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

/// A pane is a purely presentational viewport.
///
/// It owns geometry and window placement. It does not own bytes, a link, or a
/// tmux handle. `buffer_id` is only a UI association to independently managed
/// bytes.
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
    /// Buffer displayed by this viewport. `None` is an empty UI pane.
    #[serde(default)]
    pub buffer_id: Option<BufferId>,
    /// Currently parent window (None = orphan pane, hidden). Set by
    /// `xscore pane add-to-window --pane <pid> --window <wid>`; cleared
    /// by `xscore pane remove-from-window`.
    pub window_id: Option<WindowId>,
}

/// Bytes are owned by a buffer, never by a pane or a link.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct BufferInfo {
    pub id: BufferId,
    pub session_id: SessionId,
    pub name: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub origin: BufferOrigin,
}

/// Input source interpreted by a backend session manager.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum BufferOrigin {
    Link {
        link_id: LinkId,
    },
    Tmux {
        link_id: LinkId,
        pane_handle: String,
    },
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
    /// A RawSessionManager routes pipe Links into Buffers. The daemon owns the
    /// live transports and buffer pumps.
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
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TmuxHostConfig {
    #[default]
    Local,
    Ssh {
        destination: String,
        args: Vec<String>,
    },
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
            backend: SessionBackend::Tmux {
                host,
                socket,
                session: session.into(),
            },
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
            TmuxHostConfig::Ssh {
                destination: "devbox".into(),
                args: vec![],
            },
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
