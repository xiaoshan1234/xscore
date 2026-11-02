//! daemon 有线协议：长度前缀 JSON 帧。

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use crate::cli::Cmd;

/// 单帧 body 上限，防止损坏/恶意的长度前缀导致无界分配。
pub const MAX_FRAME: usize = 16 * 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Request {
    Ping,
    Status,
    Stop,
    Command { cmd: Cmd },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Response {
    pub ok: bool,
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default)]
    pub output: String,
}

impl Response {
    pub fn ok(output: String) -> Self {
        Self {
            ok: true,
            error: None,
            output,
        }
    }

    pub fn fail(error: impl Into<String>) -> Self {
        Self {
            ok: false,
            error: Some(error.into()),
            output: String::new(),
        }
    }
}

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
