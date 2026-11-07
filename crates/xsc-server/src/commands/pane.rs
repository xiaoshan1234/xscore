//! pane 命令：write / read / resize / focus / list / del / fetch。

use anyhow::{anyhow, Context, Result};

use crate::term::TerminalCore;
use xscore::domain::{TerminalId, TerminalKind};
use xscore::platform::Size;

use crate::cli::{LogFormat, TmlLogRequest};

use crate::app::App;
use crate::logging::{default_path, LoggerFormat};
use crate::render::{snapshot_text, Output};

/// 向指定 pane 写入以空格拼接的文本并补平台换行。
///
/// 参数/输出：`app`、`pane`、`text` 为输入；返回空输出，失败时返回错误。
pub async fn write(app: &App, pane: String, text: Vec<String>) -> Result<Output> {
    let id = parse_pane(app, &pane)?;
    let entry = app.terminal(id).unwrap();
    let mut data = text.join(" ").into_bytes();
    let eol: &[u8] = if entry.handle.kind() == TerminalKind::Local && cfg!(windows) {
        b"\r\n"
    } else {
        b"\n"
    };
    data.extend_from_slice(eol);
    entry.handle.send_input(&data).await?;
    Ok(Output::new())
}

/// 读取指定 pane 的当前屏幕快照。
///
/// 参数/输出：`app` 和 `pane` 为输入；返回快照文本。
pub async fn read(app: &App, pane: String) -> Result<Output> {
    let id = parse_pane(app, &pane)?;
    let entry = app.terminal(id).unwrap();
    let mut out = Output::new();
    out.push(&snapshot_text(&entry.term).await);
    Ok(out)
}

/// 调整指定 pane 及其终端网格尺寸。
///
/// 参数/输出：`app`、`pane`、`cols`、`rows` 为输入；返回空输出。
pub async fn resize(app: &App, pane: String, cols: u16, rows: u16) -> Result<Output> {
    let id = parse_pane(app, &pane)?;
    let entry = app.terminal(id).unwrap();
    entry.handle.resize(Size::new(cols, rows)).await?;
    entry.term.lock().await.resize(cols, rows);
    Ok(Output::new())
}

/// 切换本地 pane 或 tmux 容器内 pane 的焦点。
///
/// 参数/输出：`app` 为输入输出；`pane` 为输入；返回焦点提示文本。
pub fn focus(app: &mut App, pane: String) -> Result<Output> {
    let id = parse_pane(app, &pane)?;
    if let Some(window) = app.tmux.get(&id).map(|m| m.window.clone()) {
        if !app.layout.set_mirror_active(&window, id) {
            return Err(anyhow!("tmux window not found in layout: {window}"));
        }
    } else {
        app.layout.focus(id);
    }
    let mut out = Output::new();
    out.line(format!("focused: {id}"));
    Ok(out)
}

/// 关闭并删除指定 pane。
///
/// 参数/输出：`app` 为输入输出；`pane` 为输入；返回删除提示文本。
pub async fn pane_del(app: &mut App, pane: String) -> Result<Output> {
    let id = parse_pane(app, &pane)?;
    {
        let entry = app.terminal(id).unwrap();
        entry.handle.close().await?;
    }
    app.remove_terminal_local(id);
    let mut out = Output::new();
    out.line(format!("deleted pane: {id}"));
    Ok(out)
}

/// 从 tmux 拉取指定 pane 的滚动历史。
///
/// 参数/输出：`app` 和 `pane` 为输入；`line` 为可选最近行数输入；返回历史文本。
pub async fn pane_fetch(app: &App, pane: String, line: Option<usize>) -> Result<Output> {
    let id = parse_pane(app, &pane)?;
    let start = match line {
        Some(line_count) => format!("-{line_count}"),
        None => "-".to_string(),
    };
    let meta = app
        .tmux
        .get(&id)
        .ok_or_else(|| anyhow!("pane {id} is not a tmux pane"))?;
    let session = &meta.session;
    let control = app
        .tmux_control(session)
        .ok_or_else(|| anyhow!("tmux session control unavailable"))?;
    let handle = &meta.handle;
    let lines = control
        .send(&format!("capture-pane -p -S {start} -t {handle}"))
        .await?;
    let mut out = Output::new();
    for line in &lines {
        out.line(line.trim_end());
    }
    Ok(out)
}

/// 为指定 pane 创建本地输出 logger。
///
/// 参数/输出：`app` 为输入输出，`args` 为输入；返回 logger 文件和容量信息。
pub fn pane_log(app: &mut App, args: TmlLogRequest) -> Result<Output> {
    let id = parse_pane(app, &args.pane)?;
    let format = match args.format {
        LogFormat::Raw => LoggerFormat::Raw,
        LogFormat::Char => LoggerFormat::Char,
    };
    let path = match args.path {
        Some(path) => path,
        None => default_path(id, format)?,
    };
    let handle = app
        .terminal(id)
        .ok_or_else(|| anyhow!("pane not found: {id}"))?
        .handle
        .clone();
    let info = app.loggers.start(handle, format, args.max_size, path)?;

    let mut out = Output::new();
    out.line(format!(
        "logger: pane={} format={}",
        info.pane_id, info.format
    ));
    out.line(format!(
        "limit: {} {}",
        info.max_size,
        if info.format == LoggerFormat::Raw {
            "bytes"
        } else {
            "lines"
        }
    ));
    out.line(format!("file: {}", info.path.display()));
    Ok(out)
}

fn parse_pane(app: &App, s: &str) -> Result<TerminalId> {
    let id = TerminalId::new(s.parse().context("pane id")?);
    if !app.terminals.contains(id) {
        return Err(anyhow!("pane not found: {id}"));
    }
    Ok(id)
}
