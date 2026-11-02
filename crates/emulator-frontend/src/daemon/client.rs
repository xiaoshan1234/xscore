//! CLI 侧的 daemon 客户端：自动拉起、请求与响应。

use std::time::Duration;

use anyhow::{anyhow, Context, Result};

use super::ipc;
use super::protocol::{self, Request, Response};
use crate::cli::Cmd;

/// 确保有一个 daemon 在运行；没有就拉起一个并等待其就绪。
pub async fn ensure_daemon() -> Result<()> {
    if ipc::probe().await {
        return Ok(());
    }
    spawn_daemon()?;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        if ipc::probe().await {
            return Ok(());
        }
        if tokio::time::Instant::now() >= deadline {
            anyhow::bail!("daemon did not become ready in time");
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

fn spawn_daemon() -> Result<()> {
    use std::process::Stdio;

    let exe = std::env::current_exe().context("resolve daemon executable")?;
    let mut command = std::process::Command::new(exe);
    command.arg("daemon").arg("serve");
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        command.creation_flags(CREATE_NO_WINDOW | DETACHED_PROCESS);
    }
    command.spawn().context("spawn daemon")?;
    Ok(())
}

async fn request(request: &Request) -> Result<Response> {
    let mut stream = ipc::connect()
        .await
        .map_err(|error| anyhow!("daemon not running ({}): {error}", ipc::address()))?;
    protocol::write_frame(&mut *stream, request).await?;
    protocol::read_frame(&mut *stream)
        .await?
        .ok_or_else(|| anyhow!("daemon closed the connection"))
}

fn unwrap(response: Response) -> Result<String> {
    if response.ok {
        Ok(response.output)
    } else {
        Err(anyhow!(
            "{}",
            response.error.unwrap_or_else(|| "daemon error".into())
        ))
    }
}

/// 把一条命令交给 daemon 执行并返回输出文本。密码在发送前于本进程解析。
pub async fn run_command(cmd: Cmd) -> Result<String> {
    let cmd = crate::cli::resolve_secrets(cmd)?;
    ensure_daemon().await?;
    unwrap(request(&Request::Command { cmd }).await?)
}

/// 查询 daemon 当前持有的状态。
pub async fn daemon_status() -> Result<String> {
    unwrap(request(&Request::Status).await?)
}

/// 请求 daemon 退出。返回 `false` 表示本来就没有 daemon 在运行。
pub async fn daemon_stop() -> Result<bool> {
    match request(&Request::Stop).await {
        Ok(response) if response.ok => Ok(true),
        Ok(response) => Err(anyhow!(
            "{}",
            response
                .error
                .unwrap_or_else(|| "daemon refused stop".into())
        )),
        Err(_) => Ok(false),
    }
}
