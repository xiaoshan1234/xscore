//! TmuxDomain：1:N 域，一个控制连接对应 session 内的多个 pane。

use crate::domain::{PaneHandle, PaneId, PaneKind};
use crate::platform::tmux::{
    list_panes_command, list_windows_command, parse_pane_line, parse_window_line,
    resize_pane_command, send_keys_literal, TmuxControl, TmuxHostConfig, TmuxSocket,
};
use crate::platform::Size;
use crate::Result;
use async_trait::async_trait;
use std::sync::Arc;
use tokio::sync::broadcast;

/// 一个 tmux pane 的结构与几何信息。
#[derive(Debug, Clone)]
pub struct TmuxPaneInfo {
    /// tmux pane 句柄，例如 `%3`。
    pub handle: String,
    /// pane 列数。
    pub cols: u16,
    /// pane 行数。
    pub rows: u16,
    /// pane 在 window 中的左偏移。
    pub x: u16,
    /// pane 在 window 中的上偏移。
    pub y: u16,
}

/// 一个 tmux window 及其 pane 的结构信息。
#[derive(Debug, Clone)]
pub struct TmuxWindowInfo {
    /// tmux window 句柄，例如 `@1`。
    pub handle: String,
    /// window 名称。
    pub name: String,
    /// window 列数。
    pub cols: u16,
    /// window 行数。
    pub rows: u16,
    /// 该 window 是否为 session 的当前活动 window。
    pub active: bool,
    /// window 内的 pane 列表。
    pub panes: Vec<TmuxPaneInfo>,
}

/// 通过 control 客户端枚举一个 session 当前的 window/pane 结构。
///
/// 参数/输出：`control` 和 `session` 为输入；返回 window 及其 pane 的结构快照。
pub async fn enumerate(control: &TmuxControl, session: &str) -> Result<Vec<TmuxWindowInfo>> {
    let mut windows = Vec::new();
    for line in control.send(&list_windows_command(session)).await? {
        tracing::debug!(line = ?line, "tmux attach: window line");
        let Some(window) = parse_window_line(&line) else {
            continue;
        };
        let mut panes = Vec::new();
        for line in control.send(&list_panes_command(&window.handle)).await? {
            tracing::debug!(line = ?line, "tmux attach: pane line");
            let Some(pane) = parse_pane_line(&line) else {
                continue;
            };
            panes.push(TmuxPaneInfo {
                handle: pane.handle,
                cols: pane.cols,
                rows: pane.rows,
                x: pane.x,
                y: pane.y,
            });
        }
        windows.push(TmuxWindowInfo {
            handle: window.handle,
            name: window.name,
            cols: window.cols,
            rows: window.rows,
            active: window.active,
            panes,
        });
    }
    Ok(windows)
}

/// tmux 的 1:N 域入口。
///
/// 一个实例用于创建和附加 tmux session 对应的多个 pane。
#[derive(Debug, Default)]
pub struct TmuxDomain;

impl TmuxDomain {
    /// 创建一个无内部状态的 tmux 域。
    ///
    /// 参数/输出：无输入；返回新的 [`TmuxDomain`]。
    pub fn new() -> Self {
        Self
    }

    /// 连接到一个 tmux session，枚举其 window/pane，为每个 pane 返回句柄。
    ///
    /// 参数/输出：`self`、`host`、`socket` 和 `session` 为输入；返回该 session
    /// 内全部 pane 的共享句柄。
    pub async fn attach(
        &self,
        host: &TmuxHostConfig,
        socket: &TmuxSocket,
        session: &str,
    ) -> Result<Vec<Arc<TmuxPane>>> {
        let host_desc = match host {
            TmuxHostConfig::Local => "local".to_string(),
            TmuxHostConfig::Ssh {
                user, host, port, ..
            } => format!("ssh:{user}@{host}:{port}"),
            TmuxHostConfig::Wsl { .. } => "wsl".to_string(),
        };
        tracing::debug!(session, ?socket, host = %host_desc, "tmux attach");
        let control = Arc::new(TmuxControl::spawn(host, socket, session).await?);

        let windows = enumerate(&control, session).await?;
        let mut panes = Vec::new();
        for window in &windows {
            for pane in &window.panes {
                panes.push(make_pane(
                    control.clone(),
                    window.handle.clone(),
                    pane,
                    window.cols,
                    window.rows,
                    window.active,
                ));
            }
        }

        tracing::debug!(panes = panes.len(), "tmux attach: enumerated");
        Ok(panes)
    }
}

/// 为一个已存在的 control 客户端创建一个 pane 句柄并订阅其 `%output`。
///
/// 参数/输出：`control`、`window`、`pane`、`window_cols`、`window_rows` 和
/// `window_active` 为输入；返回新建的 pane 句柄，并同时启动输出转发任务。
pub fn make_pane(
    control: Arc<TmuxControl>,
    window: String,
    pane: &TmuxPaneInfo,
    window_cols: u16,
    window_rows: u16,
    window_active: bool,
) -> Arc<TmuxPane> {
    let id = PaneId::next();
    let mut rx = control.output(&pane.handle);
    let (tx, _) = broadcast::channel::<Vec<u8>>(256);
    let tx_pump = tx.clone();
    tokio::spawn(async move {
        while let Some(chunk) = rx.recv().await {
            if tx_pump.send(chunk).is_err() {
                break;
            }
        }
    });
    Arc::new(TmuxPane {
        id,
        window,
        handle: pane.handle.clone(),
        cols: pane.cols,
        rows: pane.rows,
        x: pane.x,
        y: pane.y,
        window_cols,
        window_rows,
        window_active,
        control,
        output: tx,
    })
}

/// tmux 控制连接上的逻辑 pane 句柄。
///
/// 所有输入都转换为 tmux 控制命令，输出来自对应 `%output` 订阅。
pub struct TmuxPane {
    id: PaneId,
    window: String,
    handle: String,
    cols: u16,
    rows: u16,
    x: u16,
    y: u16,
    window_cols: u16,
    window_rows: u16,
    window_active: bool,
    control: Arc<TmuxControl>,
    output: broadcast::Sender<Vec<u8>>,
}

impl TmuxPane {
    /// 返回 pane 当前记录的列数。
    ///
    /// 参数/输出：`self` 为输入；返回列数。
    pub fn cols(&self) -> u16 {
        self.cols
    }

    /// 返回 pane 当前记录的行数。
    ///
    /// 参数/输出：`self` 为输入；返回行数。
    pub fn rows(&self) -> u16 {
        self.rows
    }

    /// 返回 pane 相对 window 的左偏移。
    ///
    /// 参数/输出：`self` 为输入；返回列偏移。
    pub fn x(&self) -> u16 {
        self.x
    }

    /// 返回 pane 相对 window 的上偏移。
    ///
    /// 参数/输出：`self` 为输入；返回行偏移。
    pub fn y(&self) -> u16 {
        self.y
    }

    /// 返回所属 tmux window 的列数。
    ///
    /// 参数/输出：`self` 为输入；返回列数。
    pub fn window_cols(&self) -> u16 {
        self.window_cols
    }

    /// 返回所属 tmux window 的行数。
    ///
    /// 参数/输出：`self` 为输入；返回行数。
    pub fn window_rows(&self) -> u16 {
        self.window_rows
    }

    /// 判断所属 window 是否为当前活动 window。
    ///
    /// 参数/输出：`self` 为输入；返回活动状态。
    pub fn window_active(&self) -> bool {
        self.window_active
    }

    /// 返回所属 window 的 tmux 句柄。
    ///
    /// 参数/输出：`self` 为输入；返回句柄字符串引用。
    pub fn window(&self) -> &str {
        &self.window
    }

    /// 返回该 pane 的 tmux 句柄。
    ///
    /// 参数/输出：`self` 为输入；返回句柄字符串引用。
    pub fn tmux_handle(&self) -> &str {
        &self.handle
    }

    /// 返回共享的控制连接。
    ///
    /// 参数/输出：`self` 为输入；返回控制连接引用。
    pub fn control(&self) -> &Arc<TmuxControl> {
        &self.control
    }

    /// 直接向 tmux 控制连接发送一条命令。
    ///
    /// 参数/输出：`self` 为输入；`command` 为输入 tmux 命令；返回命令回复行。
    pub async fn send_command(&self, command: &str) -> Result<Vec<String>> {
        self.control.send(command).await
    }

    /// 重新枚举当前 session 的 window/pane 结构。
    ///
    /// 参数/输出：`self` 为输入；返回最新结构快照。
    pub async fn enumerate_windows(&self) -> Result<Vec<TmuxWindowInfo>> {
        enumerate(&self.control, self.control.session()).await
    }
}

#[async_trait]
impl PaneHandle for TmuxPane {
    fn id(&self) -> PaneId {
        self.id
    }

    fn kind(&self) -> PaneKind {
        PaneKind::Tmux
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    async fn send_input(&self, data: &[u8]) -> Result<usize> {
        let text = String::from_utf8_lossy(data);
        let command = send_keys_literal(&self.handle, &text)?;
        self.control.send(&command).await?;
        Ok(data.len())
    }

    async fn resize(&self, size: Size) -> Result<()> {
        let command = resize_pane_command(&self.handle, size.cols, size.rows);
        self.control.send(&command).await?;
        Ok(())
    }

    fn subscribe(&self) -> broadcast::Receiver<Vec<u8>> {
        self.output.subscribe()
    }

    async fn close(&self) -> Result<()> {
        self.control
            .send(&format!("kill-pane -t {}", self.handle))
            .await?;
        Ok(())
    }
}
