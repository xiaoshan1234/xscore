//! 日志初始化：写入文件（`XSCORE_LOG`，默认 `emulator.log`），stderr 只留 WARN。

use std::sync::Mutex as StdMutex;

use anyhow::{Context, Result};
use tracing_subscriber::filter::LevelFilter;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{EnvFilter, Layer};

/// 初始化文件日志与 stderr WARN 日志。
///
/// 参数/输出：无输入；成功时安装全局 tracing subscriber，否则返回错误。
pub fn init() -> Result<()> {
    let path = std::env::var("XSCORE_LOG").unwrap_or_else(|_| "emulator.log".to_string());
    let file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .with_context(|| format!("open log file {path}"))?;

    let file_layer = tracing_subscriber::fmt::layer()
        .with_writer(StdMutex::new(file))
        .with_ansi(false);
    let stderr_layer = tracing_subscriber::fmt::layer()
        .with_writer(std::io::stderr)
        .with_filter(LevelFilter::WARN);
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("emulator=debug,emulator_frontend=debug"));

    tracing_subscriber::registry()
        .with(filter)
        .with(file_layer)
        .with(stderr_layer)
        .try_init()
        .context("init tracing subscriber")?;
    Ok(())
}
