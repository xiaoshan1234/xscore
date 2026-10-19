// cli/pane.rs
use crate::core::{Error, PaneId, SessionManager, WindowId};
use clap::Subcommand;
use std::sync::Arc;

#[derive(Subcommand)]
pub enum PaneCmd {
    /// Open a new pane inside a window (tmux split-window)
    Add {
        #[arg(long)] window: String,
        /// Optional human-friendly name (not persisted)
        #[arg(long)]
        name: Option<String>,
    },

    /// Kill the tmux split + remove from registry (full teardown).
    /// Cross-invocation re-attach goes through `xscore link open`, not through panes.
    Del { #[arg(long)] id: String },

    /// Print PaneInfo JSON
    Show { #[arg(long)] id: String },

    /// List panes (optionally filtered by --window)
    List { #[arg(long)] window: Option<String> },
}

pub async fn run(mgr: Arc<SessionManager>, cmd: PaneCmd) -> anyhow::Result<()> {
    match cmd {
        PaneCmd::Add { window, name } => {
            let wid = WindowId::parse(&window)?;
            // 2-arg signature per task-06/14 reconciliation (ADR 0006).
            let pid = mgr.add_pane(wid, name).await?;
            println!("pane:  {pid}");
        }
        PaneCmd::Del { id } => {
            let pid = PaneId::parse(&id)?;
            mgr.del_pane(pid).await?;
            println!("deleted pane: {pid}");
        }
        PaneCmd::Show { id } => {
            let pid = PaneId::parse(&id)?;
            let info = mgr.panes.get(pid).await.ok_or(Error::PaneNotFound(pid))?;
            println!("{}", serde_json::to_string_pretty(&info)?);
        }
        PaneCmd::List { window } => {
            let wid_filter = window.as_deref().map(WindowId::parse).transpose()?;
            println!(
                "{:<36}  {:<36}  {:<12}",
                "PANE ID", "WINDOW ID", "SINK HANDLE"
            );
            for p in mgr.panes.list_all().await {
                if let Some(wid) = wid_filter {
                    if p.window_id != wid { continue; }
                }
                println!("{:<36}  {:<36}  {:<12}", p.id, p.window_id, p.sink_handle);
            }
        }
    }
    Ok(())
}