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
        let control = Arc::new(TmuxControl::spawn(&prefix, socket, session)?);
        let mut panes = Vec::new();

        for line in control.send(&list_windows_command(session)).await? {
            let Some(window) = parse_window_line(&line) else {
                continue;
            };
            for line in control.send(&list_panes_command(&window.handle)).await? {
                let Some(pane) = parse_pane_line(&line) else {
                    continue;
                };
                panes.push(self.make_pane(control.clone(), pane.handle, pane.cols, pane.rows));
            }
        }

        Ok(panes)
    }

    fn make_pane(
        &self,
        control: Arc<TmuxControl>,
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
            handle,
            cols,
            rows,
            control,
            output: tx,
        })
    }
}

pub struct TmuxPane {
    id: PaneId,
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
}

#[async_trait]
impl PaneHandle for TmuxPane {
    fn id(&self) -> PaneId {
        self.id
    }

    fn kind(&self) -> PaneKind {
        PaneKind::Tmux
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
        // pane 关闭只摘除订阅；session 由控制连接整体持有。
        Ok(())
    }
}
