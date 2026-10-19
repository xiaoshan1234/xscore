use crate::core::{Error, LinkId, Result};
use crate::transport::Transport;
use crate::util::tmux_arg_socket;
use async_trait::async_trait;
use tokio::process::Command as TokioCommand;

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
    /// (used during state hydration — task-15). The session is NOT
    /// created here; the caller must have already verified it exists
    /// via `tmux has-session -t <session>`.
    pub fn adopt(link_id: LinkId, mode: &str, session_name: &str) -> Self {
        Self { link_id, mode: mode.into(), session_name: session_name.into() }
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

    async fn new_pane(&self) -> Result<String> {
        // v0.3.0: panes always go into the link's single tmux window (window_idx 0).
        // The sink owns the tmux session; the link's tmux-session-window-count is 1.
        //
        // Three-step discovery for robustness against tmux's internal race:
        //   1. `tmux split-window -P -F "#{pane_id}"` — ask tmux to print the new
        //      pane id. Output is informational; we don't trust it.
        //   2. Sleep 50ms (heuristic; tmux server async race window).
        //   3. `tmux list-panes` — query the actual pane list. The newly-created
        //      pane is the one with the highest pane_index.
        let addr = format!("{}:0", self.session_name);
        let split_out = self.tmux()
            .args(["split-window", "-t", &addr, "-P", "-F", "#{pane_id}"])
            .output().await?;
        let split_stdout = String::from_utf8_lossy(&split_out.stdout).trim().to_string();
        let split_stderr = String::from_utf8_lossy(&split_out.stderr).trim().to_string();
        if !split_out.status.success() {
            return Err(Error::Sink(format!(
                "tmux split-window: status={:?} stdout={split_stdout:?} stderr={split_stderr:?}",
                split_out.status
            )));
        }

        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        let list_out = self.tmux()
            .args(["list-panes", "-t", &addr, "-F", "#{pane_id}\t#{pane_index}"])
            .output().await?;
        let list_stdout = String::from_utf8_lossy(&list_out.stdout);
        if !list_out.status.success() {
            return Err(Error::Sink(format!(
                "tmux list-panes after split: status={:?} stderr={:?}",
                list_out.status,
                String::from_utf8_lossy(&list_out.stderr)
            )));
        }
        let mut best: Option<(u32, String)> = None;
        for line in list_stdout.lines() {
            if line.is_empty() { continue; }
            let mut parts = line.splitn(2, '\t');
            let pid_str = parts.next().unwrap_or("");
            let idx_str = parts.next().unwrap_or("0");
            if let (Ok(idx), true) = (idx_str.parse::<u32>(), !pid_str.is_empty()) {
                let candidate = (idx, pid_str.to_string());
                best = Some(match best {
                    Some(prev) if prev.0 >= idx => prev,
                    _ => candidate,
                });
            }
        }
        match best {
            Some((_, pane_id)) => Ok(pane_id),
            None => {
                if !split_stdout.is_empty() {
                    Ok(split_stdout)
                } else {
                    Err(Error::Sink(format!(
                        "tmux new_pane: split printed {split_stdout:?} but list-panes empty; \
                         session={addr} (likely tmux race — see TmuxSink::new_pane)"
                    )))
                }
            }
        }
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