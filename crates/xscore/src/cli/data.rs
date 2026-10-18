// cli/data.rs
//
// `xscore data {read, write, resize}` — per-pane data ops (ADR 0006).
// Pane-level granularity is the only place `pane-id` is needed. Read
// pulls a tmux `capture-pane` snapshot; write sends text to the tmux
// session; resize adjusts the pane dimensions (forwarded to the
// transport's resize, which for tmux is `resize-pane -t <session>:<idx>`).

use crate::core::PaneId;
use crate::core::SessionManager;
use clap::Subcommand;
use std::sync::Arc;

#[derive(Subcommand)]
pub enum DataCmd {
    /// Read recent bytes from a pane (snapshot via tmux capture-pane)
    Read {
        #[arg(long)] pane: String,
        #[arg(long, default_value_t = 4096)] bytes: usize,
    },
    /// Write bytes to a pane (default: read from stdin; or `--from-file <path>`)
    Write {
        #[arg(long)] pane: String,
        /// Source file (use `-` for stdin — but stdin is the default anyway)
        #[arg(long)] from_file: Option<String>,
    },
    /// Resize a pane (forwarded to transport.resize + sink.resize)
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
            let backend = mgr.backend(pid).await?;
            let mut buf = vec![0u8; bytes];
            let n = backend.snapshot(&mut buf).await?;
            use std::io::Write;
            std::io::stdout().write_all(&buf[..n])?;
        }
        DataCmd::Write { pane, from_file } => {
            let pid = PaneId::parse(&pane)?;
            let backend = mgr.backend(pid).await?;
            let data = if let Some(p) = from_file {
                std::fs::read(p)?
            } else {
                use std::io::Read;
                let mut s = Vec::new();
                std::io::stdin().read_to_end(&mut s)?;
                s
            };
            let n = backend.write(&data).await?;
            eprintln!("wrote {n} bytes");
        }
        DataCmd::Resize { pane, cols, rows } => {
            let pid = PaneId::parse(&pane)?;
            let backend = mgr.backend(pid).await?;
            backend.resize(cols, rows).await?;
            eprintln!("resized to {cols}x{rows}");
        }
    }
    Ok(())
}
