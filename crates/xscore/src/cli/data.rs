// cli/data.rs (v0.4.0 — data ops route through `pane.bridge`)
//
// `xscore data {read, write, resize}` — per-pane data ops. Read serves a
// snapshot of the pane's buffer (direct panes: the ring buffer; tmux
// panes: `tmux capture-pane`). Write sends text to the bridge. Resize
// forwards to the bridge and updates the pane info.

use crate::core::PaneId;
use crate::core::SessionManager;
use clap::Subcommand;
use std::sync::Arc;

#[derive(Subcommand)]
pub enum DataCmd {
    /// Read a snapshot of the pane's buffer (bytes `--bytes` wide).
    /// Direct panes: the ring buffer. tmux panes: capture-pane.
    /// Errors with `NotAttached(pid)` for orphan panes.
    Read {
        #[arg(long)] pane: String,
        #[arg(long, default_value_t = 4096)] bytes: usize,
    },
    /// Write bytes to a pane's bridge (default: read from stdin; or `--from-file <path>`).
    Write {
        #[arg(long)] pane: String,
        #[arg(long)] from_file: Option<String>,
    },
    /// Resize a pane's terminal dimensions (forwards to bridge if attached).
    Resize {
        #[arg(long)] pane: String,
        #[arg(long)] cols: u16,
        #[arg(long)] rows: u16,
    },
}

pub async fn run(mgr: Arc<SessionManager>, cmd: DataCmd) -> anyhow::Result<()> {
    match cmd {
        DataCmd::Read { pane, bytes } => {
            let pid = PaneId::parse(&pane)?;
            let mut buf = vec![0u8; bytes];
            let n = mgr.data_read(pid, &mut buf).await?;
            use std::io::Write;
            std::io::stdout().write_all(&buf[..n])?;
        }
        DataCmd::Write { pane, from_file } => {
            let pid = PaneId::parse(&pane)?;
            let data = if let Some(p) = from_file {
                std::fs::read(p)?
            } else {
                use std::io::Read;
                let mut s = Vec::new();
                std::io::stdin().read_to_end(&mut s)?;
                s
            };
            let n = mgr.data_write(pid, &data).await?;
            eprintln!("wrote {n} bytes");
        }
        DataCmd::Resize { pane, cols, rows } => {
            let pid = PaneId::parse(&pane)?;
            mgr.data_resize(pid, cols, rows).await?;
            eprintln!("resized to {cols}x{rows}");
        }
    }
    Ok(())
}