//! Shared transport contract between the `xsc` client and the resident daemon.
//!
//! The client forwards the raw command argv over a framed loopback TCP
//! connection; all command interpretation lives in the daemon (`xsc-server`).
//! This crate only defines the request/response envelope, the framed codec,
//! and the per-user loopback port.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

/// Wire protocol version understood by this build.
pub const PROTOCOL_VERSION: u32 = 2;

/// Maximum serialized frame size.
pub const MAX_FRAME: usize = 16 * 1024 * 1024;

/// Request understood by the daemon.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Request {
    /// Negotiate the wire protocol before sending another request.
    Hello {
        /// Protocol version implemented by the client.
        protocol_version: u32,
    },
    /// Probe daemon liveness.
    Ping,
    /// Read daemon state.
    Status,
    /// Stop the daemon.
    Stop,
    /// Execute one terminal-management command.
    Command {
        /// Raw command argv; parsed by the daemon.
        argv: Vec<String>,
    },
}

/// Generic response returned by the daemon.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Response {
    /// Whether the request succeeded.
    pub ok: bool,
    /// Error text when the request failed.
    #[serde(default)]
    pub error: Option<String>,
    /// Text output when the request succeeded.
    #[serde(default)]
    pub output: String,
}

impl Response {
    /// Construct a successful response.
    pub fn ok(output: String) -> Self {
        Self {
            ok: true,
            error: None,
            output,
        }
    }

    /// Construct a failed response.
    pub fn fail(error: impl Into<String>) -> Self {
        Self {
            ok: false,
            error: Some(error.into()),
            output: String::new(),
        }
    }
}

/// Derive the per-user loopback port used by the daemon.
pub fn port() -> u16 {
    #[cfg(windows)]
    let identity = std::env::var("USERPROFILE").unwrap_or_default();
    #[cfg(not(windows))]
    let identity = std::env::var("HOME").unwrap_or_default();
    let hash = fnv1a(identity.as_bytes());
    40_000 + (hash % 20_000) as u16
}

fn fnv1a(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

/// Connect to the current user's daemon.
pub async fn connect() -> std::io::Result<tokio::net::TcpStream> {
    let stream = tokio::net::TcpStream::connect(("127.0.0.1", port())).await?;
    stream.set_nodelay(true)?;
    Ok(stream)
}

/// Check whether the daemon port accepts TCP connections.
pub async fn probe() -> bool {
    connect().await.is_ok()
}

/// Write one length-prefixed JSON frame.
pub async fn write_frame<W>(writer: &mut W, value: &impl Serialize) -> Result<()>
where
    W: AsyncWrite + Unpin + ?Sized,
{
    let body = serde_json::to_vec(value).context("serialize frame")?;
    if body.len() > MAX_FRAME {
        anyhow::bail!("frame body too large: {} bytes", body.len());
    }
    writer
        .write_all(&(body.len() as u32).to_le_bytes())
        .await
        .context("write frame length")?;
    writer.write_all(&body).await.context("write frame body")?;
    Ok(())
}

/// Read one length-prefixed JSON frame.
pub async fn read_frame<R, T>(reader: &mut R) -> Result<Option<T>>
where
    R: AsyncRead + Unpin + ?Sized,
    T: for<'de> Deserialize<'de>,
{
    let mut len_bytes = [0u8; 4];
    let mut filled = 0usize;
    while filled < 4 {
        let n = reader
            .read(&mut len_bytes[filled..])
            .await
            .context("read frame length")?;
        if n == 0 {
            return Ok(None);
        }
        filled += n;
    }
    let len = u32::from_le_bytes(len_bytes) as usize;
    if len > MAX_FRAME {
        anyhow::bail!("frame body too large: {len} bytes");
    }
    let mut body = vec![0u8; len];
    reader
        .read_exact(&mut body)
        .await
        .context("read frame body")?;
    let value = serde_json::from_slice(&body).context("deserialize frame")?;
    Ok(Some(value))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn frames_roundtrip() {
        let (mut client, mut server) = tokio::io::duplex(4096);
        write_frame(&mut client, &Request::Ping)
            .await
            .expect("write request");
        let request: Request = read_frame(&mut server)
            .await
            .expect("read request")
            .expect("request frame");
        assert!(matches!(request, Request::Ping));
    }
}
