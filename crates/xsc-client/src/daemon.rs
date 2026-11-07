//! CLI 侧的 daemon 客户端：自动拉起、请求与响应。

use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use xsc_protocol::{self as protocol, Request, Response, PROTOCOL_VERSION};

/// 确保有一个 daemon 在运行；没有就拉起一个并等待其就绪。
///
/// 参数/输出：无输入；成功时无返回值，启动或等待失败时返回错误。
pub async fn ensure_daemon() -> Result<()> {
    if protocol::probe().await {
        return Ok(());
    }
    spawn_daemon()?;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        if protocol::probe().await {
            return Ok(());
        }
        if tokio::time::Instant::now() >= deadline {
            anyhow::bail!("daemon did not become ready in time");
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

fn spawn_daemon() -> Result<()> {
    let exe = daemon_executable()?;
    let mut command = std::process::Command::new(exe);
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

fn daemon_executable() -> Result<PathBuf> {
    if let Some(path) = std::env::var_os("XSCORE_DAEMON_EXE") {
        let path = PathBuf::from(path);
        if path.is_file() {
            return Ok(path);
        }
        anyhow::bail!(
            "XSCORE_DAEMON_EXE does not point to a file: {}",
            path.display()
        );
    }

    let current = std::env::current_exe().context("resolve client executable")?;
    let candidate = current
        .parent()
        .ok_or_else(|| anyhow!("client executable has no parent directory"))?
        .join(if cfg!(windows) {
            "xs-daemon.exe"
        } else {
            "xs-daemon"
        });
    if candidate.is_file() {
        Ok(candidate)
    } else {
        Err(anyhow!(
            "daemon executable not found at {} (install `xsc-server` beside `xs` or set XSCORE_DAEMON_EXE)",
            candidate.display()
        ))
    }
}

async fn request(request: &Request) -> Result<Response> {
    let mut stream = protocol::connect()
        .await
        .map_err(|error| anyhow!("daemon not running (port {}): {error}", protocol::port()))?;

    protocol::write_frame(
        &mut stream,
        &Request::Hello {
            protocol_version: PROTOCOL_VERSION,
        },
    )
    .await?;
    let hello: Response = protocol::read_frame(&mut stream)
        .await?
        .ok_or_else(|| anyhow!("daemon closed during protocol negotiation"))?;
    if !hello.ok {
        return Err(anyhow!(
            "{}",
            hello
                .error
                .unwrap_or_else(|| "daemon rejected protocol version".into())
        ));
    }
    if hello.output != PROTOCOL_VERSION.to_string() {
        return Err(anyhow!(
            "unsupported daemon protocol response: {}",
            hello.output
        ));
    }

    protocol::write_frame(&mut stream, request).await?;
    protocol::read_frame(&mut stream)
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

/// 把原始 argv 交给 daemon 执行并返回输出文本。
///
/// 参数/输出：`argv` 为输入；返回 daemon 输出文本。
pub async fn run_command(argv: Vec<String>) -> Result<String> {
    ensure_daemon().await?;
    unwrap(request(&Request::Command { argv }).await?)
}

/// Start the server executable in the current terminal and wait for it to exit.
pub fn serve_foreground() -> Result<()> {
    let status = std::process::Command::new(daemon_executable()?)
        .status()
        .context("start daemon")?;
    if status.success() {
        Ok(())
    } else {
        Err(anyhow!("daemon exited with {status}"))
    }
}

/// Dispatch a daemon lifecycle subcommand from raw argv.
///
/// 参数/输出：`argv` 为输入，首项为 `daemon`，次项为 `serve`、`status` 或
/// `stop`；返回命令结果。
pub async fn run_bootstrap(argv: &[String]) -> Result<()> {
    match argv.get(1).map(String::as_str) {
        Some("serve") => serve_foreground(),
        Some("status") => {
            let output = daemon_status().await?;
            print!("{output}");
            Ok(())
        }
        Some("stop") => match daemon_stop().await? {
            true => {
                println!("daemon stopped");
                Ok(())
            }
            false => {
                println!("daemon not running");
                Ok(())
            }
        },
        Some(other) => Err(anyhow!("unknown daemon subcommand: {other}")),
        None => Err(anyhow!("daemon requires a subcommand: serve|status|stop")),
    }
}

/// 查询 daemon 当前持有的状态。
///
/// 参数/输出：无输入；返回状态文本。
pub async fn daemon_status() -> Result<String> {
    unwrap(request(&Request::Status).await?)
}

/// 请求 daemon 退出。返回 `false` 表示本来就没有 daemon 在运行。
///
/// 参数/输出：无输入；返回是否实际停止了一个 daemon。
pub async fn daemon_stop() -> Result<bool> {
    if !protocol::probe().await {
        return Ok(false);
    }
    let response = request(&Request::Stop).await?;
    if response.ok {
        Ok(true)
    } else {
        Err(anyhow!(
            "{}",
            response
                .error
                .unwrap_or_else(|| "daemon refused stop".into())
        ))
    }
}
