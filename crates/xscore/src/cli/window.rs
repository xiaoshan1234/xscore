// cli/window.rs
use crate::core::{Error, LinkId, SessionManager, WindowId};
use crate::core::window::Window;
use clap::{Args, Subcommand};
use std::sync::Arc;

#[derive(Subcommand)]
pub enum WindowCmd {
    /// Create a new tmux window inside the link's session
    Add(WindowAddArgs),
    /// List windows (optionally filtered by --link)
    List { #[arg(long)] link: Option<String> },
    /// Print WindowInfo JSON
    Show { #[arg(long)] id: String },
    /// tmux kill-window (panes inside die with it; no in-memory detach semantic)
    Del { #[arg(long)] id: String },
}

#[derive(Args)]
pub struct WindowAddArgs {
    /// Link to add this window to
    #[arg(long)] link: String,
    /// Window name (becomes `tmux -L xscore new-window -t <session> -n <name>`)
    #[arg(long)] name: Option<String>,
}

pub async fn run(mgr: Arc<SessionManager>, cmd: WindowCmd) -> anyhow::Result<()> {
    match cmd {
        WindowCmd::Add(args) => add(mgr, args).await,
        WindowCmd::List { link } => list(mgr, link).await,
        WindowCmd::Show { id } => show(mgr, &id).await,
        WindowCmd::Del { id } => del(mgr, &id).await,
    }
}

async fn add(mgr: Arc<SessionManager>, args: WindowAddArgs) -> anyhow::Result<()> {
    let lid = LinkId::parse(&args.link)?;
    let wid = mgr.add_window(lid, args.name).await?;
    println!("window: {wid}");
    Ok(())
}

async fn list(mgr: Arc<SessionManager>, link: Option<String>) -> anyhow::Result<()> {
    let wid_list: Vec<Arc<Window>> = match link {
        Some(l) => {
            let lid = LinkId::parse(&l)?;
            mgr.windows.list_for_link(lid).await
        }
        None => {
            let mut all = vec![];
            for arc_link in mgr.links.list().await {
                for w in mgr.windows.list_for_link(arc_link.id).await {
                    all.push(w);
                }
            }
            all
        }
    };
    for w in wid_list {
        println!("{}\t{}\t{}\t{}", w.info.id, w.info.link_id, w.info.name, w.info.tmux_window_index);
    }
    Ok(())
}

async fn show(mgr: Arc<SessionManager>, id: &str) -> anyhow::Result<()> {
    let wid = WindowId::parse(id)?;
    let w = mgr.windows.get(wid).await.ok_or(Error::WindowNotFound(wid))?;
    println!("{}", serde_json::to_string_pretty(&w.info)?);
    Ok(())
}

async fn del(mgr: Arc<SessionManager>, id: &str) -> anyhow::Result<()> {
    let wid = WindowId::parse(id)?;
    mgr.del_window(wid).await?;
    println!("deleted window: {wid}");
    Ok(())
}