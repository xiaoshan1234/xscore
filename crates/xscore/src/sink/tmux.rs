// sink/tmux.rs (v0.4.0 — tmux adoption, ADR 0008)
//
// TmuxSink owns a tmux SESSION. Everything inside it (windows, panes)
// is created by tmux and reported back by tmux — xscore never invents a
// `%N`/`@N` handle, and always stores the geometry tmux reports.
//
// All tmux invocations go through `tmux_arg_socket()` so they hit the
// shared `xscore` server (or `$XSCORE_SOCKET`), never the user's
// default tmux server.
use crate::core::{Error, LinkId, Result};
use crate::transport::Transport;
use crate::util::tmux_arg_socket;
use async_trait::async_trait;
use tokio::process::Command as TokioCommand;

use super::{TmuxPaneSnapshot, TmuxWindowSnapshot};

/// Format string used for windows: handle, name, width, height, layout.
const WINDOW_FMT: &str = "#{window_id}\t#{window_name}\t#{window_width}\t#{window_height}\t#{window_layout}";
/// Format string used for panes: handle, width, height, left, top, index.
const PANE_FMT: &str = "#{pane_id}\t#{pane_width}\t#{pane_height}\t#{pane_left}\t#{pane_top}\t#{pane_index}";

#[derive(Debug)]
pub struct TmuxSink {
    pub link_id: LinkId,             // bookkeeping (used in error messages + hydrate log)
    pub mode: String,                // "local" | "remote" (post-resolution)
    pub session_name: String,        // tmux session name == the link key
}

impl TmuxSink {
    pub async fn create_link(
        link_id: LinkId,
        mode: &str,
        session_name: &str,
        transport: &mut dyn Transport,
    ) -> Result<Self> {
        transport.open().await?;
        let mut args = tmux_arg_socket();
        args.extend([
            "new-session".into(), "-d".into(), "-s".into(), session_name.into(),
            "-x".into(), "120".into(), "-y".into(), "30".into(),
        ]);
        let status = TokioCommand::new("tmux").args(&args).status().await?;
        if !status.success() {
            return Err(Error::Sink(format!("tmux new-session: exit {status}")));
        }
        Ok(Self { link_id, mode: mode.into(), session_name: session_name.into() })
    }

    fn tmux(&self) -> TokioCommand {
        let mut c = TokioCommand::new("tmux");
        c.args(tmux_arg_socket());
        c
    }

    /// Construct a `TmuxSink` that wraps an EXISTING tmux session
    /// (used during state hydration). The session is NOT created here;
    /// the caller must have already verified it exists via
    /// `tmux has-session -t <session>`.
    pub fn adopt(link_id: LinkId, mode: &str, session_name: &str) -> Self {
        Self { link_id, mode: mode.into(), session_name: session_name.into() }
    }

    /// Run a tmux subcommand and return trimmed stdout, mapping a
    /// non-zero exit to `Error::Sink` with the command's stderr.
    async fn run(&self, args: &[&str]) -> Result<String> {
        let out = self.tmux().args(args).output().await?;
        if !out.status.success() {
            return Err(Error::Sink(format!(
                "tmux {}: status={:?} stderr={:?}",
                args.first().copied().unwrap_or(""),
                out.status,
                String::from_utf8_lossy(&out.stderr).trim()
            )));
        }
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    }

    /// Parse one `WINDOW_FMT` line.
    fn parse_window(line: &str) -> Option<TmuxWindowSnapshot> {
        let mut p = line.splitn(5, '\t');
        let handle = p.next()?.trim();
        if handle.is_empty() {
            return None;
        }
        let name = p.next().unwrap_or("").to_string();
        let cols = p.next().and_then(|s| s.trim().parse().ok()).unwrap_or(0);
        let rows = p.next().and_then(|s| s.trim().parse().ok()).unwrap_or(0);
        let layout = p.next().unwrap_or("").to_string();
        Some(TmuxWindowSnapshot { handle: handle.to_string(), name, cols, rows, layout })
    }

    /// Parse one `PANE_FMT` line.
    fn parse_pane(line: &str) -> Option<TmuxPaneSnapshot> {
        let mut p = line.splitn(6, '\t');
        let handle = p.next()?.trim();
        if handle.is_empty() {
            return None;
        }
        let cols = p.next().and_then(|s| s.trim().parse().ok()).unwrap_or(0);
        let rows = p.next().and_then(|s| s.trim().parse().ok()).unwrap_or(0);
        let x = p.next().and_then(|s| s.trim().parse().ok()).unwrap_or(0);
        let y = p.next().and_then(|s| s.trim().parse().ok()).unwrap_or(0);
        let index = p.next().and_then(|s| s.trim().parse().ok()).unwrap_or(0);
        Some(TmuxPaneSnapshot { handle: handle.to_string(), cols, rows, x, y, index })
    }

    /// Re-read one pane's geometry directly from tmux. Used as the fallback
    /// when `list-panes` races the split (tmux server is async).
    async fn pane_snapshot(&self, pane_handle: &str) -> Result<TmuxPaneSnapshot> {
        let out = self.run(&["display-message", "-p", "-t", pane_handle, PANE_FMT]).await?;
        Self::parse_pane(&out).ok_or_else(|| {
            Error::Sink(format!("tmux display-message returned unparsable pane {out:?}"))
        })
    }
}

#[async_trait]
impl super::Sink for TmuxSink {
    fn kind(&self) -> &'static str { "tmux" }

    async fn attach(&self, _transport: &dyn Transport) -> Result<()> {
        // tmux already running on its own socket; attach is reserved for
        // ssh-channel wiring. No-op for now.
        Ok(())
    }

    async fn list_windows(&self) -> Result<Vec<TmuxWindowSnapshot>> {
        let out = self.run(&["list-windows", "-t", &self.session_name, "-F", WINDOW_FMT]).await?;
        Ok(out.lines().filter_map(Self::parse_window).collect())
    }

    async fn list_panes(&self, window_handle: &str) -> Result<Vec<TmuxPaneSnapshot>> {
        let out = self.run(&["list-panes", "-t", window_handle, "-F", PANE_FMT]).await?;
        Ok(out.lines().filter_map(Self::parse_pane).collect())
    }

    async fn new_window(&self, name: &str) -> Result<TmuxWindowSnapshot> {
        let out = self.run(&[
            "new-window", "-t", &self.session_name, "-n", name, "-P", "-F", WINDOW_FMT,
        ]).await?;
        Self::parse_window(&out).ok_or_else(|| {
            Error::Sink(format!("tmux new-window returned unparsable window {out:?}"))
        })
    }

    async fn kill_window(&self, window_handle: &str) -> Result<()> {
        self.run(&["kill-window", "-t", window_handle]).await.map(|_| ())
    }

    async fn split_pane(&self, window_handle: &str) -> Result<TmuxPaneSnapshot> {
        // Ask tmux to print the new pane id, then (after tmux's async
        // window settles) re-read the window's pane list and take the
        // highest pane_index — the same race workaround as v0.3.0, but
        // now scoped to an explicit window and returning real geometry.
        let printed = self.run(&[
            "split-window", "-t", window_handle, "-P", "-F", "#{pane_id}",
        ]).await?;

        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        if let Ok(panes) = self.list_panes(window_handle).await {
            if let Some(best) = panes.into_iter().max_by_key(|p| p.index) {
                return Ok(best);
            }
        }
        if printed.is_empty() {
            return Err(Error::Sink(format!(
                "tmux split-window on {window_handle} printed no pane id \
                 (likely tmux race — see TmuxSink::split_pane)"
            )));
        }
        // list-panes raced us; ask tmux about the pane we just created.
        self.pane_snapshot(&printed).await
    }

    async fn close_pane(&self, pane_handle: &str) -> Result<()> {
        self.run(&["kill-pane", "-t", pane_handle]).await.map(|_| ())
    }

    async fn snapshot(&self, pane_handle: &str, buf: &mut [u8]) -> Result<usize> {
        let out = self.tmux()
            .args(["capture-pane", "-p", "-t", pane_handle, "-J"])
            .output().await?;
        if !out.status.success() {
            return Err(Error::Sink(format!(
                "capture-pane exit {:?}: {}",
                out.status,
                String::from_utf8_lossy(&out.stderr).trim()
            )));
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
        self.run(&[
            "resize-pane", "-t", pane_handle, "-x", &cols.to_string(), "-y", &rows.to_string(),
        ]).await.map(|_| ())
    }

    async fn close(&self) -> Result<()> {
        // kill-session takes every window and pane with it — this is the
        // single teardown path for a tmux link.
        let _ = self.tmux()
            .args(["kill-session", "-t", &self.session_name])
            .status().await?;
        Ok(())
    }
}
