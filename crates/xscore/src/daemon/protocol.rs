// daemon/protocol.rs (v0.5.0 — resident daemon, ADR 0009)
//
// Wire format: local IPC (Unix socket or Windows named pipe),
// **length-prefixed JSON**:
//
//     [u32 little-endian body length][JSON UTF-8 body]
//
// Binary payloads (`Read` results, `Write` payloads) travel as **hex
// strings** inside the JSON. No new crates: hex is written by hand below
// (ADR 0009 § "Protocol" explains why base64 was rejected — one less
// dependency for a 20-line helper).
//
// Requests are a serde-tagged enum (`op` discriminator). The
// `TransportConfig` payload is the *existing* model type so the daemon
// speaks exactly the same JSON shape that `state.json` persists — there
// is no second transport vocabulary to keep in sync.
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use crate::infra::link::TransportConfig;
use crate::infra::tmux::{TmuxHostConfig, TmuxSocket};
use crate::service::buffer_service::BufferId;
use crate::service::link_service::LinkId;
use crate::service::session_service::SessionId;
use crate::{Error, Result};

/// Hard cap on a single frame body. A pane buffer is 64 KiB and the
/// largest legitimate request is a `write` of that size (128 KiB as hex),
/// so 16 MiB is generous while still refusing to allocate unbounded
/// memory on a corrupt/hostile length prefix.
pub const MAX_FRAME: usize = 16 * 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Request {
    /// Liveness check. Answers `{"ok":true}`.
    Ping,
    /// Snapshot of what the daemon holds.
    Status,
    /// Create (or replace) a link's live transport inside the daemon.
    CreateLink {
        link: LinkId,
        transport: TransportConfig,
    },
    /// Close a link's transport and unbind its panes. Pane buffers stay.
    DeleteLink { link: LinkId },
    /// Ensure a buffer slot exists. Idempotent.
    CreateBuffer { buffer: BufferId },
    /// Drop a buffer slot and its bytes. Idempotent.
    DeleteBuffer { buffer: BufferId },
    /// Open (or reuse) the control client of a tmux session and answer with
    /// its structure. The daemon — not the CLI — owns the `tmux -CC attach`
    /// process from here on, which is what makes `%output %N` a stream that
    /// outlives the CLI (ADR 0009 §1, ADR 0010 §D2).
    ///
    /// `cols`/`rows` are the size the caller saw; `-C attach` takes no size,
    /// so they are informational (a fresh client adopts the session's own).
    OpenTmux {
        session: SessionId,
        host: TmuxHostConfig,
        socket: TmuxSocket,
        name: String,
        cols: u16,
        rows: u16,
    },
    /// Enumerate a tmux session's windows and panes **through its control
    /// client** (one round trip per listing, no per-command process). Takes
    /// the slot's stored config, so it re-opens a client that died.
    Structure { session: SessionId },
    /// Run one command on the session's control client and return the lines
    /// of its `%begin…%end` block. Used for the mutations that must still go
    /// through the one client (`new-window`, `split-window`, `kill-pane`) —
    /// never for data or structure.
    TmuxCommand { session: SessionId, command: String },
    /// Close the session's control client and drop its pane buffers.
    CloseTmux { session: SessionId },
    /// Is the session's control client alive? (`link show`'s `connected`.)
    ProbeTmux { session: SessionId },
    /// Bind a buffer to a link and start its pump.
    ///
    /// `key: Some("%N")` means "this buffer is fed by `%N` on `session`'s
    /// control client": subscribe `%output %N` into that buffer. It is a no-op
    /// when the pane is already attached — a second subscriber would append
    /// every byte twice. `key: None` is the native pipe path below.
    Attach {
        buffer: BufferId,
        #[serde(default)]
        link: Option<LinkId>,
        #[serde(default)]
        key: Option<String>,
        #[serde(default)]
        session: Option<SessionId>,
    },
    /// Stop a buffer's pump. The buffer (and the link) survive.
    Detach { buffer: BufferId },
    /// Non-destructive snapshot read of the buffer.
    Read { buffer: BufferId, max: usize },
    /// Write bytes (hex-encoded) to the buffer's attached transport.
    Write { buffer: BufferId, data: String },
    /// Resize the buffer's attached transport.
    Resize {
        buffer: BufferId,
        cols: u16,
        rows: u16,
    },
    /// Ask the daemon to exit (it removes its socket on the way out).
    Stop,
}

/// `buffered` is the number of bytes currently in the pane's ring
/// buffer — enough for `xscore daemon status` to be diagnosable without
/// a separate op.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct BufferStatus {
    pub buffer: BufferId,
    /// Attached link id, or `""` when the pane is detached. Empty string
    /// (not null) keeps the field a plain string as specified in ADR
    /// 0009's protocol table.
    pub link: String,
    pub buffered: usize,
}

pub use crate::service::backend::tmux_model::{TmuxPaneRow, TmuxStructure, TmuxWindowRow};
/// One tmux control slot, as `daemon status` reports it: enough to see that
/// the daemon is holding a client and buffering bytes.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TmuxStatus {
    pub session: SessionId,
    /// The tmux session name on its host.
    pub name: String,
    /// The client is up and tmux has not sent `%exit`.
    pub alive: bool,
    /// Panes with a `%output` subscriber (their buffers are being filled).
    pub panes: usize,
    /// Total bytes held in those panes' buffers.
    pub buffered: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Status {
    /// The daemon's PID — handy for `kill -TERM` when debugging.
    pub pid: u32,
    pub links: Vec<LinkId>,
    #[serde(default)]
    pub buffers: Vec<BufferStatus>,
    /// One entry per tmux session the daemon holds a control client for.
    #[serde(default)]
    pub tmux: Vec<TmuxStatus>,
}

/// One response shape for every op. Unused fields are omitted so the
/// simple answers stay `{"ok":true}`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Response {
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub n: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<Status>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub structure: Option<TmuxStructure>,
    /// The answer to `ProbeTmux`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub alive: Option<bool>,
    /// The answer to `TmuxCommand` — the block's own lines, verbatim.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lines: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl Response {
    pub fn ok() -> Self {
        Self {
            ok: true,
            data: None,
            n: None,
            status: None,
            structure: None,
            alive: None,
            lines: None,
            error: None,
        }
    }

    pub fn bytes(bytes: &[u8]) -> Self {
        Self {
            data: Some(to_hex(bytes)),
            ..Self::ok()
        }
    }

    pub fn count(n: usize) -> Self {
        Self {
            n: Some(n),
            ..Self::ok()
        }
    }

    pub fn status(status: Status) -> Self {
        Self {
            status: Some(status),
            ..Self::ok()
        }
    }

    pub fn structure(structure: TmuxStructure) -> Self {
        Self {
            structure: Some(structure),
            ..Self::ok()
        }
    }

    /// The `ProbeTmux` answer.
    pub fn bool(alive: bool) -> Self {
        Self {
            alive: Some(alive),
            ..Self::ok()
        }
    }

    /// The `TmuxCommand` answer.
    pub fn lines(lines: Vec<String>) -> Self {
        Self {
            lines: Some(lines),
            ..Self::ok()
        }
    }

    pub fn err(message: impl Into<String>) -> Self {
        Self {
            ok: false,
            error: Some(message.into()),
            ..Self::ok()
        }
    }
}

// ===== hex =====

pub fn to_hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        // Writing into a String is infallible.
        let _ = write!(s, "{b:02x}");
    }
    s
}

pub fn from_hex(s: &str) -> Result<Vec<u8>> {
    let bytes = s.as_bytes();
    if !bytes.len().is_multiple_of(2) {
        return Err(Error::Invalid(format!(
            "hex payload has odd length ({})",
            bytes.len()
        )));
    }
    let mut out = Vec::with_capacity(bytes.len() / 2);
    for pair in bytes.chunks(2) {
        let hi = (pair[0] as char)
            .to_digit(16)
            .ok_or_else(|| Error::Invalid(format!("invalid hex digit {:?}", pair[0] as char)))?;
        let lo = (pair[1] as char)
            .to_digit(16)
            .ok_or_else(|| Error::Invalid(format!("invalid hex digit {:?}", pair[1] as char)))?;
        out.push(((hi << 4) | lo) as u8);
    }
    Ok(out)
}

// ===== framing =====

/// Write one length-prefixed JSON frame.
pub async fn write_frame<W, T>(w: &mut W, value: &T) -> std::io::Result<()>
where
    W: AsyncWrite + Unpin + ?Sized,
    T: Serialize,
{
    let body = serde_json::to_vec(value)?;
    if body.len() > MAX_FRAME {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("frame too large: {} bytes", body.len()),
        ));
    }
    w.write_all(&(body.len() as u32).to_le_bytes()).await?;
    w.write_all(&body).await?;
    w.flush().await?;
    Ok(())
}

/// Read one length-prefixed JSON frame. `Ok(None)` means the peer closed
/// the connection cleanly between frames (normal for the one-shot client
/// connections this protocol uses).
pub async fn read_frame<R, T>(r: &mut R) -> std::io::Result<Option<T>>
where
    R: AsyncRead + Unpin + ?Sized,
    T: serde::de::DeserializeOwned,
{
    let mut len_buf = [0u8; 4];
    match r.read_exact(&mut len_buf).await {
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(e) => return Err(e),
    }
    let len = u32::from_le_bytes(len_buf) as usize;
    if len > MAX_FRAME {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("frame too large: {len} bytes"),
        ));
    }
    let mut body = vec![0u8; len];
    r.read_exact(&mut body).await?;
    let value = serde_json::from_slice(&body).map_err(|e| {
        std::io::Error::new(std::io::ErrorKind::InvalidData, format!("bad frame: {e}"))
    })?;
    Ok(Some(value))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_roundtrip_including_nul_and_high_bytes() {
        let raw: Vec<u8> = vec![0x00, 0x01, 0x7f, 0x80, 0xfe, 0xff, b'a', b'\n'];
        let encoded = to_hex(&raw);
        assert_eq!(encoded, "00017f80feff610a");
        assert_eq!(from_hex(&encoded).unwrap(), raw);
    }

    #[test]
    fn hex_empty_is_empty() {
        assert_eq!(to_hex(&[]), "");
        assert!(from_hex("").unwrap().is_empty());
    }

    #[test]
    fn hex_rejects_odd_length_and_bad_digits() {
        assert!(from_hex("abc").is_err());
        let err = format!("{}", from_hex("zz").unwrap_err());
        assert!(err.contains("invalid hex digit"), "got: {err}");
    }

    #[test]
    fn request_json_shape_matches_adr() {
        let lid = LinkId::parse("01928e0a-7d10-7000-8000-000000000000").unwrap();
        let req = Request::CreateLink {
            link: lid,
            transport: TransportConfig::Shell {
                command: "/bin/bash".into(),
                args: vec![],
                cwd: None,
            },
        };
        let v = serde_json::to_value(&req).unwrap();
        assert_eq!(v["op"], serde_json::json!("create_link"));
        assert_eq!(v["link"], serde_json::json!(lid.to_string()));
        // The transport is the model's own serde shape, not a copy.
        assert_eq!(v["transport"]["transport"], serde_json::json!("shell"));
        assert_eq!(v["transport"]["command"], serde_json::json!("/bin/bash"));
        // ... and it deserializes back into the same enum.
        let back: Request = serde_json::from_value(v).unwrap();
        assert_eq!(back, req);
    }

    #[test]
    fn all_ops_use_snake_case_discriminators() {
        let pid = BufferId::new();
        let lid = LinkId::new();
        let cases = [
            (Request::Ping, "ping"),
            (Request::Status, "status"),
            (Request::DeleteLink { link: lid }, "delete_link"),
            (Request::CreateBuffer { buffer: pid }, "create_buffer"),
            (Request::DeleteBuffer { buffer: pid }, "delete_buffer"),
            (
                Request::Attach {
                    buffer: pid,
                    link: Some(lid),
                    key: None,
                    session: None,
                },
                "attach",
            ),
            (Request::Detach { buffer: pid }, "detach"),
            (
                Request::Read {
                    buffer: pid,
                    max: 10,
                },
                "read",
            ),
            (
                Request::Write {
                    buffer: pid,
                    data: "00".into(),
                },
                "write",
            ),
            (
                Request::Resize {
                    buffer: pid,
                    cols: 80,
                    rows: 24,
                },
                "resize",
            ),
            (
                Request::CloseTmux {
                    session: SessionId::new(),
                },
                "close_tmux",
            ),
            (
                Request::Structure {
                    session: SessionId::new(),
                },
                "structure",
            ),
            (
                Request::ProbeTmux {
                    session: SessionId::new(),
                },
                "probe_tmux",
            ),
            (Request::Stop, "stop"),
        ];
        for (req, op) in cases {
            let v = serde_json::to_value(&req).unwrap();
            assert_eq!(v["op"], serde_json::json!(op), "wrong op for {req:?}");
        }
    }

    #[test]
    fn open_tmux_and_command_carry_their_fields() {
        let sid = SessionId::new();
        let open = Request::OpenTmux {
            session: sid,
            host: TmuxHostConfig::Local,
            socket: TmuxSocket::Named("x".into()),
            name: "work".into(),
            cols: 100,
            rows: 30,
        };
        let v = serde_json::to_value(&open).unwrap();
        assert_eq!(v["op"], serde_json::json!("open_tmux"));
        assert_eq!(v["name"], serde_json::json!("work"));
        assert_eq!(v["socket"]["kind"], serde_json::json!("named"));
        assert_eq!(v["session"], serde_json::json!(sid.to_string()));

        let cmd = Request::TmuxCommand {
            session: sid,
            command: "new-window".into(),
        };
        let v = serde_json::to_value(&cmd).unwrap();
        assert_eq!(v["op"], serde_json::json!("tmux_command"));
        assert_eq!(v["command"], serde_json::json!("new-window"));

        // The pane-addressing fields are optional on the wire: a native
        // attach never grew them, and neither does an older caller.
        let native: Request = serde_json::from_value(serde_json::json!({
            "op": "attach",
            "buffer": BufferId::new().to_string(),
            "link": LinkId::new().to_string(),
        }))
        .unwrap();
        match native {
            Request::Attach {
                link, key, session, ..
            } => {
                assert!(link.is_some());
                assert!(key.is_none() && session.is_none());
            }
            other => panic!("expected attach, got {other:?}"),
        }
    }

    #[test]
    fn structure_and_probe_answers_roundtrip() {
        let sid = SessionId::new();
        let structure = TmuxStructure {
            windows: vec![TmuxWindowRow {
                handle: "@1".into(),
                name: "sh".into(),
                cols: 100,
                rows: 30,
            }],
            panes: vec![TmuxPaneRow {
                handle: "%0".into(),
                cols: 100,
                rows: 30,
                x: 0,
                y: 0,
                window_handle: "@1".into(),
            }],
        };
        let resp = Response::structure(structure.clone());
        let back: Response = serde_json::from_str(&serde_json::to_string(&resp).unwrap()).unwrap();
        assert_eq!(back.structure, Some(structure));
        assert!(back.alive.is_none() && back.lines.is_none());

        let probe = Response::bool(false);
        let back: Response = serde_json::from_str(&serde_json::to_string(&probe).unwrap()).unwrap();
        assert_eq!(back.alive, Some(false));
        assert!(back.ok);

        // A status with no tmux slots still deserializes (older daemons).
        let old: Status = serde_json::from_str(r#"{"pid":1,"links":[],"panes":[]}"#).unwrap();
        assert!(old.tmux.is_empty());

        let with_tmux = Status {
            pid: 1,
            links: vec![],
            buffers: vec![],
            tmux: vec![TmuxStatus {
                session: sid,
                name: "work".into(),
                alive: true,
                panes: 1,
                buffered: 3,
            }],
        };
        let back: Status =
            serde_json::from_str(&serde_json::to_string(&with_tmux).unwrap()).unwrap();
        assert_eq!(back, with_tmux);
    }

    #[test]
    fn bare_ok_response_is_a_single_field() {
        let s = serde_json::to_string(&Response::ok()).unwrap();
        assert_eq!(s, r#"{"ok":true}"#);
    }

    #[test]
    fn error_response_roundtrips() {
        let resp = Response::err("boom");
        let s = serde_json::to_string(&resp).unwrap();
        assert!(
            s.contains(r#""ok":false"#) && s.contains(r#""error":"boom""#),
            "{s}"
        );
        let back: Response = serde_json::from_str(&s).unwrap();
        assert_eq!(back, resp);
        assert!(back.status.is_none() && back.data.is_none());
    }

    #[tokio::test]
    async fn frames_roundtrip_over_a_pipe() {
        let (mut a, mut b) = tokio::io::duplex(4096);
        let req = Request::Write {
            buffer: BufferId::new(),
            data: to_hex(b"hello\n"),
        };
        write_frame(&mut a, &req).await.unwrap();
        let got: Request = read_frame(&mut b).await.unwrap().expect("frame");
        assert_eq!(got, req);

        // Clean EOF (peer dropped) reports None rather than an error.
        drop(a);
        let none: Option<Request> = read_frame(&mut b).await.unwrap();
        assert!(none.is_none());
    }

    #[test]
    fn oversized_length_prefix_is_rejected() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let (mut a, mut b) = tokio::io::duplex(64);
            use tokio::io::AsyncWriteExt;
            a.write_all(&((MAX_FRAME + 1) as u32).to_le_bytes())
                .await
                .unwrap();
            let err = read_frame::<_, Request>(&mut b).await.unwrap_err();
            assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
        });
    }
}
