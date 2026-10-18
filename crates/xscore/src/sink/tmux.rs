use crate::core::{Error, LinkId, Result};
use crate::model::WindowInfo;
use crate::transport::Transport;
use crate::util::tmux_arg_socket;
use async_trait::async_trait;
use tokio::process::Command;

pub struct TmuxSink {
    pub link_id: LinkId,             // used to populate WindowInfo.link_id on new_window
    pub mode: String,                // "local" | "remote" (post-resolution)
    pub session_name: String,        // tmux session name == the link key (session lives in shared -L xscore server)
}

impl TmuxSink {
    pub async fn create(link_id: LinkId, mode: &str, session_name: &str, transport: &mut dyn Transport) -> Result<Self> {
        transport.open().await?;
        let mut args = tmux_arg_socket();
        args.extend([
            "new-session".into(), "-d".into(), "-s".into(), session_name.into(),
            "-x".into(), "120".into(), "-y".into(), "30".into(),
        ]);
        let status = Command::new("tmux").args(&args).status().await?;
        if !status.success() {
            return Err(Error::Sink(format!("tmux new-session: exit {status}")));
        }
        Ok(Self { link_id, mode: mode.into(), session_name: session_name.into() })
    }

    fn tmux(&self) -> Command {
        let mut c = Command::new("tmux");
        c.args(tmux_arg_socket());
        c
    }

    fn session_window_addr(&self, idx: u32) -> String {
        format!("{}:{}", self.session_name, idx)
    }
}

#[async_trait]
impl super::Sink for TmuxSink {
    fn kind(&self) -> &'static str { "tmux" }

    async fn attach(&self, _transport: &dyn Transport) -> Result<()> {
        // tmux already running on its own socket; attach is reserved for ssh-channel
        // wiring in the SSH-integration PR. No-op for now.
        Ok(())
    }

    async fn new_window(&self, name: &str) -> Result<WindowInfo> {
        // Use `#{window_index}` (not `#{window_id}`) — `window_id` is a
        // STABLE unique ID assigned at creation (so the 2nd window ever
        // created in a tmux server is `@2`, not `@1`); using it as an
        // index in `tmux split-window -t <session>:<idx>` fails because
        // the index is the window's 0-based position, not its ID. The
        // fix is to query the index directly. Caveat: indices shift when
        // windows are removed; for our use case (kill_window by index),
        // we operate on the window we just created, so the index is
        // stable enough.
        let out = self.tmux()
            .args(["new-window", "-t", &self.session_name, "-n", name,
                   "-P", "-F", "#{window_index}"])
            .output().await?;
        if !out.status.success() {
            return Err(Error::Sink(format!("tmux new-window: {:?}", out.status)));
        }
        let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
        let tmux_window_index: u32 = s.trim_start_matches('@').parse()
            .map_err(|e| Error::Sink(format!("parse window_id {s:?}: {e}")))?;
        Ok(WindowInfo {
            id: crate::core::WindowId::new(),
            link_id: self.link_id,
            name: name.to_string(),
            created_at: chrono::Utc::now(),
            tmux_window_index,
        })
    }

    async fn kill_window(&self, tmux_window_index: u32) -> Result<()> {
        let addr = self.session_window_addr(tmux_window_index);
        let status = self.tmux()
            .args(["kill-window", "-t", &addr])
            .status().await?;
        if !status.success() {
            return Err(Error::Sink(format!("kill-window exit {status}")));
        }
        Ok(())
    }

    async fn new_pane(&self, tmux_window_index: u32) -> Result<String> {
        // Pane = a tmux split inside the named window.
        let addr = self.session_window_addr(tmux_window_index);
        let out = self.tmux()
            .args(["split-window", "-t", &addr, "-P", "-F", "#{pane_id}"])
            .output().await?;
        if !out.status.success() {
            return Err(Error::Sink(format!("tmux split-window: {:?}", out.status)));
        }
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    }

    async fn close_pane(&self, pane_handle: &str) -> Result<()> {
        let status = self.tmux()
            .args(["kill-pane", "-t", pane_handle])
            .status().await?;
        if !status.success() {
            return Err(Error::Sink(format!("kill-pane exit {status}")));
        }
        Ok(())
    }

    async fn list_panes(&self) -> Result<Vec<String>> {
        let out = self.tmux()
            .args(["list-panes", "-t", &self.session_name, "-F", "#{pane_id}"])
            .output().await?;
        if !out.status.success() {
            return Err(Error::Sink(format!("list-panes exit {:?}", out.status)));
        }
        Ok(String::from_utf8_lossy(&out.stdout)
            .lines()
            .filter(|l| !l.is_empty())
            .map(String::from)
            .collect())
    }

    async fn snapshot(&self, pane_handle: &str, buf: &mut [u8]) -> Result<usize> {
        let out = self.tmux()
            .args(["capture-pane", "-p", "-t", pane_handle, "-J"])
            .output().await?;
        if !out.status.success() {
            return Err(Error::Sink(format!("capture-pane exit {:?}", out.status)));
        }
        let bytes = &out.stdout;
        let n = bytes.len().min(buf.len());
        buf[..n].copy_from_slice(&bytes[..n]);
        Ok(n)
    }

    async fn write(&self, pane_handle: &str, data: &[u8]) -> Result<usize> {
        let s = std::str::from_utf8(data).map_err(|e| Error::Invalid(e.to_string()))?;
        let mut cmd = self.tmux();
        cmd.arg("send-keys").arg("-t").arg(pane_handle);
        for ch in s.chars() { cmd.arg(ch.to_string()); }
        let status = cmd.status().await?;
        if !status.success() { return Err(Error::Sink(format!("send-keys exit {status}"))); }
        Ok(data.len())
    }

    async fn resize(&self, pane_handle: &str, cols: u16, rows: u16) -> Result<()> {
        let status = self.tmux()
            .args(["resize-pane", "-t", pane_handle,
                   "-x", &cols.to_string(), "-y", &rows.to_string()])
            .status().await?;
        if !status.success() {
            return Err(Error::Sink(format!("resize-pane exit {status}")));
        }
        Ok(())
    }

    async fn close(&self) -> Result<()> {
        let _ = self.tmux()
            .args(["kill-session", "-t", &self.session_name])
            .status().await?;
        Ok(())
    }
}