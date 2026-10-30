//! TmuxDomain：1:N 域，一个控制连接对应 session 内的多个 pane。

use crate::domain::{PaneHandle, PaneId, PaneKind};
use crate::platform::tmux::{
    host_prefix, list_panes_command, list_windows_command, parse_pane_line, parse_window_line,
    resize_pane_command, send_keys_literal, TmuxControl, TmuxHostConfig, TmuxSocket,
};
use crate::platform::Size;
use crate::Result;
use async_trait::async_trait;
use std::sync::Arc;
use tokio::sync::broadcast;

#[derive(Debug, Clone)]
pub struct TmuxPaneInfo {
    pub handle: String,
    pub cols: u16,
    pub rows: u16,
}

#[derive(Debug, Clone)]
pub struct TmuxWindowInfo {
    pub handle: String,
    pub name: String,
    pub panes: Vec<TmuxPaneInfo>,
}

/// 通过 control 客户端枚举一个 session 当前的 window/pane 结构。
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
            });
        }
        windows.push(TmuxWindowInfo {
            handle: window.handle,
            name: window.name,
            panes,
        });
    }
    Ok(windows)
}

#[derive(Debug, Default)]
pub struct TmuxDomain;

impl TmuxDomain {
    pub fn new() -> Self {
        Self
    }

    /// 连接到一个 tmux session，枚举其 window/pane，为每个 pane 返回句柄。
    pub async fn attach(
        &self,
        host: &TmuxHostConfig,
        socket: &TmuxSocket,
        session: &str,
    ) -> Result<Vec<Arc<TmuxPane>>> {
        let prefix = host_prefix(host);
        let host_desc = match host {
            TmuxHostConfig::Local => "local".to_string(),
            TmuxHostConfig::Ssh { destination, .. } => format!("ssh:{destination}"),
            TmuxHostConfig::Wsl { .. } => "wsl".to_string(),
        };
        tracing::debug!(session, ?socket, host = %host_desc, "tmux attach");
        let (password, askpass) = match host {
            TmuxHostConfig::Ssh {
                password, askpass, ..
            } => (password.as_deref(), askpass.as_deref()),
            _ => (None, None),
        };
        let use_pty = !matches!(host, TmuxHostConfig::Ssh { .. });
        let control = Arc::new(TmuxControl::spawn(
            &prefix, socket, session, password, askpass, use_pty,
        )?);

        let windows = enumerate(&control, session).await?;
        let mut panes = Vec::new();
        for window in &windows {
            for pane in &window.panes {
                panes.push(make_pane(
                    control.clone(),
                    window.handle.clone(),
                    pane.handle.clone(),
                    pane.cols,
                    pane.rows,
                ));
            }
        }

        tracing::debug!(panes = panes.len(), "tmux attach: enumerated");
        Ok(panes)
    }
}

/// 为一个已存在的 control 客户端创建一个 pane 句柄并订阅其 `%output`。
pub fn make_pane(
    control: Arc<TmuxControl>,
    window: String,
    handle: String,
    cols: u16,
    rows: u16,
) -> Arc<TmuxPane> {
    let id = PaneId::next();
    let mut rx = control.output(&handle);
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
        handle,
        cols,
        rows,
        control,
        output: tx,
    })
}

pub struct TmuxPane {
    id: PaneId,
    window: String,
    handle: String,
    cols: u16,
    rows: u16,
    control: Arc<TmuxControl>,
    output: broadcast::Sender<Vec<u8>>,
}

impl TmuxPane {
    pub fn cols(&self) -> u16 {
        self.cols
    }

    pub fn rows(&self) -> u16 {
        self.rows
    }

    pub fn window(&self) -> &str {
        &self.window
    }

    pub fn tmux_handle(&self) -> &str {
        &self.handle
    }

    pub fn control(&self) -> &Arc<TmuxControl> {
        &self.control
    }

    pub async fn send_command(&self, command: &str) -> Result<Vec<String>> {
        self.control.send(command).await
    }

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
