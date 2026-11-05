//! daemon 有线协议：长度前缀 JSON 帧。

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use crate::cli::Cmd;

/// 单帧 body 上限，防止损坏/恶意的长度前缀导致无界分配。
pub const MAX_FRAME: usize = 16 * 1024 * 1024;

/// daemon 请求类型。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Request {
    /// 探活请求。
    Ping,
    /// 查询 daemon 状态。
    Status,
    /// 请求 daemon 退出。
    Stop,
    /// 执行一条 CLI 命令。
    Command {
        /// 要执行的 CLI 命令。
        cmd: Cmd,
    },
}

/// daemon 的通用响应。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Response {
    /// 请求是否成功。
    pub ok: bool,
    /// 失败时的错误文本。
    #[serde(default)]
    pub error: Option<String>,
    /// 成功时的输出文本。
    #[serde(default)]
    pub output: String,
}

impl Response {
    /// 构造成功响应。
    ///
    /// 参数/输出：`output` 为输入；返回成功响应。
    pub fn ok(output: String) -> Self {
        Self {
            ok: true,
            error: None,
            output,
        }
    }

    /// 构造失败响应。
    ///
    /// 参数/输出：`error` 为输入；返回失败响应。
    pub fn fail(error: impl Into<String>) -> Self {
        Self {
            ok: false,
            error: Some(error.into()),
            output: String::new(),
        }
    }
}

/// 写入一个长度前缀 JSON 帧。
///
/// 参数/输出：`writer` 为输入输出；`value` 为输入；成功时无返回值。
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

/// 读取一个长度前缀 JSON 帧。
///
/// 参数/输出：`reader` 为输入输出；正常 EOF 返回 `None`，否则返回反序列化值。
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
