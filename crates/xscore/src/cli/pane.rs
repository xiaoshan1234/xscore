// cli/pane.rs (v0.3.0 — pane is orphan-creatable + freely switchable between links/windows)
use crate::core::{Error, PaneId, SessionManager, WindowId};
use clap::{Args, Subcommand};
use std::sync::Arc;

#[derive(Subcommand)]
pub enum PaneCmd {
    /// Create a new orphan pane (no link, no window).
    Add(PaneAddArgs),

    /// Delete the pane (full teardown — also detaches from link + removes from window).
    Del { #[arg(long)] id: String },

    /// Print PaneInfo JSON.
    Show { #[arg(long)] id: String },

    /// List panes (optional --session <s>, --window <wid> or --link <lid>
    /// filter).
    List(PaneListArgs),

    /// Bind the pane to a pipe link. Creates the bridge; the link's previous
    /// pane (if any) is auto-detached, because a pipe carries one reader.
    Attach { #[arg(long)] pane: String, #[arg(long)] link: String },

    /// Unbind the pane from its link. Closes the bridge. Pane survives as
    /// orphan (keeps its buffer content).
    Detach { #[arg(long)] pane: String },

    /// Resize the pane's terminal dimensions (forwards to bridge if attached).
    Resize(PaneResizeArgs),

    /// Update the pane's x/y position (pure UI).
    Move(PaneMoveArgs),

    /// Put the pane in a window.
    AddToWindow(PaneWindowArgs),

    /// Remove the pane from a window (pane survives as orphan).
    RemoveFromWindow(PaneWindowArgs),
}

#[derive(Args)]
pub struct PaneAddArgs {
    /// Optional human-friendly name.
    #[arg(long)]
    pub name: Option<String>,
    #[arg(long, default_value_t = 80)] pub cols: u16,
    #[arg(long, default_value_t = 24)] pub rows: u16,
    #[arg(long, default_value_t = 0)] pub x: u16,
    #[arg(long, default_value_t = 0)] pub y: u16,
    /// Put the pane in this window. If the window mirrors a tmux window,
    /// the pane is a real tmux split and tmux's geometry is used.
    #[arg(long, value_name = "WINDOW_ID")]
    pub window: Option<String>,
    /// Put the pane in this session. Omitted: the manager resolves the default
    /// (the only session, else `default`, created on demand).
    #[arg(long, value_name = "NAME")]
    pub session: Option<String>,
}

#[derive(Args)]
pub struct PaneListArgs {
    /// Only this session's panes.
    #[arg(long, value_name = "NAME")]
    pub session: Option<String>,
    #[arg(long)] pub window: Option<String>,
    #[arg(long)] pub link: Option<String>,
}

#[derive(Args)]
pub struct PaneResizeArgs {
    #[arg(long)] pub pane: String,
    #[arg(long)] pub cols: u16,
    #[arg(long)] pub rows: u16,
}

#[derive(Args)]
pub struct PaneMoveArgs {
    #[arg(long)] pub pane: String,
    #[arg(long)] pub x: u16,
    #[arg(long)] pub y: u16,
}

#[derive(Args)]
pub struct PaneWindowArgs {
    #[arg(long)] pub pane: String,
    #[arg(long)] pub window: String,
}

pub async fn run(mgr: Arc<SessionManager>, cmd: PaneCmd) -> anyhow::Result<()> {
    match cmd {
        PaneCmd::Add(args) => add(mgr, args).await,
        PaneCmd::Del { id } => del(mgr, &id).await,
        PaneCmd::Show { id } => show(mgr, &id).await,
        PaneCmd::List(args) => list(mgr, args).await,
        PaneCmd::Attach { pane, link } => attach(mgr, &pane, &link).await,
        PaneCmd::Detach { pane } => detach(mgr, &pane).await,
        PaneCmd::Resize(args) => resize(mgr, args).await,
        PaneCmd::Move(args) => move_pane(mgr, args).await,
        PaneCmd::AddToWindow(args) => add_to_window(mgr, args).await,
        PaneCmd::RemoveFromWindow(args) => remove_from_window(mgr, args).await,
    }
}

async fn add(mgr: Arc<SessionManager>, args: PaneAddArgs) -> anyhow::Result<()> {
    let window = match args.window.as_deref() {
        Some(s) => Some(WindowId::parse(s)?),
        None => None,
    };
    let sid = mgr.resolve_session(args.session.as_deref()).await?;
    let pid = mgr
        .add_pane_in(sid, args.name, args.cols, args.rows, args.x, args.y, window)
        .await?;
    println!("pane:  {pid}");
    Ok(())
}

async fn del(mgr: Arc<SessionManager>, id: &str) -> anyhow::Result<()> {
    let pid = PaneId::parse(id)?;
    mgr.del_pane(pid).await?;
    println!("deleted pane: {pid}");
    Ok(())
}

async fn show(mgr: Arc<SessionManager>, id: &str) -> anyhow::Result<()> {
    let pid = PaneId::parse(id)?;
    let info = mgr.panes.get(pid).await.ok_or(Error::PaneNotFound(pid))?;
    println!("{}", serde_json::to_string_pretty(&info)?);
    Ok(())
}

async fn list(mgr: Arc<SessionManager>, args: PaneListArgs) -> anyhow::Result<()> {
    // Same reason as `window ls`: a tmux session's panes are tmux's, and with a
    // process per command the only way to see one created by someone else is to
    // attach for the duration of the command (ADR 0010 §7).
    let only: Option<crate::core::SessionId> = match args.session.as_deref() {
        Some(name) => Some(mgr.resolve_session(Some(name)).await?),
        None => None,
    };
    for session in mgr.list_sessions().await? {
        if only.is_some_and(|id| id != session.id) {
            continue;
        }
        if session.is_tmux() {
            if let Err(e) = mgr.refresh_tmux_session(session.id).await {
                tracing::debug!("pane ls: could not refresh tmux session {}: {e}", session.name);
            }
        }
    }
    let wid_filter = match args.window.as_deref() {
        Some(s) => Some(WindowId::parse(s)?),
        None => None,
    };
    let lid_filter = match args.link.as_deref() {
        Some(s) => Some(crate::core::LinkId::parse(s)?),
        None => None,
    };
    println!("{:<36}  {:<36}  {:<36}  {:<4} {:>4} {:>4} {:>3} {:>3}",
        "PANE ID", "LINK ID", "WINDOW ID", "COLS", "ROWS", "POS", "X", "Y");
    let panes = mgr.panes.list_all().await;
    let mut sorted = panes.iter().collect::<Vec<_>>();
    sorted.sort_by_key(|p| p.id.to_string());
    for p in sorted {
        if let Some(sid) = only {
            if p.session_id != sid { continue; }
        }
        if let Some(wid) = wid_filter {
            if p.window_id != Some(wid) { continue; }
        }
        if let Some(lid) = lid_filter {
            if p.link_id != Some(lid) { continue; }
        }
        let link_id_str = match p.link_id {
            Some(l) => l.to_string(),
            None => "-".to_string(),
        };
        let window_id_str = match p.window_id {
            Some(w) => w.to_string(),
            None => "-".to_string(),
        };
        println!("{:<36}  {:<36}  {:<36}  {:<4} {:>4} {:>4} {:>3} {:>3}",
            p.id,
            link_id_str,
            window_id_str,
            p.cols,
            p.rows,
            format!("{}x{}", p.x, p.y),
            p.x,
            p.y);
    }
    Ok(())
}

async fn attach(mgr: Arc<SessionManager>, pane: &str, link: &str) -> anyhow::Result<()> {
    let pid = PaneId::parse(pane)?;
    let lid = crate::core::LinkId::parse(link)?;
    mgr.attach_pane(pid, lid).await?;
    println!("attached: {pid} → link {lid}");
    Ok(())
}

async fn detach(mgr: Arc<SessionManager>, pane: &str) -> anyhow::Result<()> {
    let pid = PaneId::parse(pane)?;
    mgr.detach_pane(pid).await?;
    println!("detached: {pid}");
    Ok(())
}

async fn resize(mgr: Arc<SessionManager>, args: PaneResizeArgs) -> anyhow::Result<()> {
    let pid = PaneId::parse(&args.pane)?;
    mgr.resize_pane(pid, args.cols, args.rows).await?;
    println!("resized {pid} to {}x{}", args.cols, args.rows);
    Ok(())
}

async fn move_pane(mgr: Arc<SessionManager>, args: PaneMoveArgs) -> anyhow::Result<()> {
    let pid = PaneId::parse(&args.pane)?;
    mgr.move_pane(pid, args.x, args.y).await?;
    println!("moved {pid} to ({}, {})", args.x, args.y);
    Ok(())
}

async fn add_to_window(mgr: Arc<SessionManager>, args: PaneWindowArgs) -> anyhow::Result<()> {
    let pid = PaneId::parse(&args.pane)?;
    let wid = WindowId::parse(&args.window)?;
    mgr.add_pane_to_window(wid, pid).await?;
    println!("pane {pid} → window {wid}");
    Ok(())
}

async fn remove_from_window(mgr: Arc<SessionManager>, args: PaneWindowArgs) -> anyhow::Result<()> {
    let pid = PaneId::parse(&args.pane)?;
    let wid = WindowId::parse(&args.window)?;
    mgr.remove_pane_from_window(wid, pid).await?;
    println!("pane {pid} ✗ window {wid}");
    Ok(())
}