// app/cli/window.rs (v0.6.0 — a window belongs to a session; tmux sessions get real
// tmux windows, native ones a purely xscore-side window)
use crate::service::session_service::SessionId;
use crate::service::session_service::SessionService;
use crate::ui::window::Window;
use crate::ui::window::WindowBackend;
use crate::ui::WindowId;
use crate::Error;
use clap::{Args, Subcommand};
use std::sync::Arc;

#[derive(Subcommand)]
pub enum WindowCmd {
    /// Create a window in a session: a real tmux window when the session is
    /// tmux-backed, a native one otherwise.
    Add(WindowAddArgs),
    /// List windows, optionally only one session's.
    List(WindowListArgs),
    /// Print WindowInfo JSON.
    Show {
        #[arg(long)]
        id: String,
    },
    /// Tear down a window row. Panes inside survive as orphans.
    Del {
        #[arg(long)]
        id: String,
    },
    /// Add a pane to this window.
    AddPane(WindowPaneArgs),
    /// Remove a pane from this window (pane stays alive as orphan).
    RemovePane(WindowPaneArgs),
}

#[derive(Args)]
pub struct WindowAddArgs {
    /// Window name (becomes `w-<first8 of window-id>` if absent; for a tmux
    /// session, tmux's own default name).
    #[arg(long)]
    pub name: Option<String>,
    /// Total window width in cells (native windows only — tmux reports its own).
    #[arg(long, default_value_t = 120)]
    pub cols: u16,
    /// Total window height in cells (native windows only — tmux reports its own).
    #[arg(long, default_value_t = 30)]
    pub rows: u16,
    /// Session that owns the window.
    #[arg(long, value_name = "NAME")]
    pub session: String,
}

#[derive(Args)]
pub struct WindowListArgs {
    /// Only this session's windows.
    #[arg(long, value_name = "NAME")]
    pub session: Option<String>,
}

#[derive(Args)]
pub struct WindowPaneArgs {
    #[arg(long)]
    pub window: String,
    #[arg(long)]
    pub pane: String,
}

pub async fn run(mgr: Arc<SessionService>, cmd: WindowCmd) -> anyhow::Result<()> {
    match cmd {
        WindowCmd::Add(args) => add(mgr, args).await,
        WindowCmd::List(args) => list(mgr, args).await,
        WindowCmd::Show { id } => show(mgr, &id).await,
        WindowCmd::Del { id } => del(mgr, &id).await,
        WindowCmd::AddPane(args) => add_pane(mgr, args).await,
        WindowCmd::RemovePane(args) => remove_pane(mgr, args).await,
    }
}

async fn add(mgr: Arc<SessionService>, args: WindowAddArgs) -> anyhow::Result<()> {
    let sid = mgr.resolve_session(Some(&args.session)).await?;
    let wid = mgr
        .add_window_in(sid, args.name, args.cols, args.rows)
        .await?;
    println!("window: {wid}");
    Ok(())
}

async fn list(mgr: Arc<SessionService>, args: WindowListArgs) -> anyhow::Result<()> {
    // ADR 0010 §7: a tmux session's structure is tmux's. This CLI is one
    // process per command, so between commands nothing is attached and a window
    // created outside xscore would be invisible; listing re-reads it through
    // the session's control client. A failure is not fatal: listing must not
    // start failing because one remote host went away (arch.md §6).
    let only: Option<SessionId> = match args.session.as_deref() {
        Some(name) => Some(mgr.resolve_session(Some(name)).await?),
        None => None,
    };
    for session in mgr.list_sessions().await? {
        if only.is_some_and(|id| id != session.id) {
            continue;
        }
        if session.is_tmux() {
            if let Err(e) = mgr.refresh_tmux_session(session.id).await {
                tracing::debug!(
                    "window ls: could not refresh tmux session {}: {e}",
                    session.name
                );
            }
        }
    }

    let names = session_names(&mgr).await;
    println!(
        "{:<36}  {:<24}  {:<20}  {:<5}  {:<5}  {:<8}",
        "WINDOW ID", "NAME", "SESSION", "COLS", "ROWS", "TMUX"
    );
    let mut all: Vec<Arc<Window>> = mgr.ui.windows().await;
    // Stable order by id (uuid v7 is chronological).
    all.sort_by_key(|w| w.info.id.to_string());
    for w in all {
        if let Some(sid) = only {
            if w.info.session_id != sid {
                continue;
            }
        }
        let tmux = match &w.info.backend {
            WindowBackend::Native => "-".to_string(),
            WindowBackend::Tmux { handle } => handle.clone(),
        };
        println!(
            "{:<36}  {:<24}  {:<20}  {:<5}  {:<5}  {:<8}",
            w.info.id,
            w.info.name,
            names.label(&w.info.session_id),
            w.info.cols,
            w.info.rows,
            tmux
        );
    }
    Ok(())
}

/// Session id → name, for the `SESSION` column. Built once per listing rather
/// than per row (every row would otherwise be a registry lookup).
struct SessionNames(Vec<(SessionId, String)>);

impl SessionNames {
    fn label(&self, id: &SessionId) -> String {
        self.0
            .iter()
            .find(|(sid, _)| sid == id)
            .map(|(_, name)| name.clone())
            .unwrap_or_else(|| id.to_string()[..8].to_string())
    }
}

async fn session_names(mgr: &SessionService) -> SessionNames {
    SessionNames(
        mgr.list_sessions()
            .await
            .unwrap_or_default()
            .into_iter()
            .map(|info| (info.id, info.name))
            .collect(),
    )
}

async fn show(mgr: Arc<SessionService>, id: &str) -> anyhow::Result<()> {
    let wid = WindowId::parse(id)?;
    let w = mgr.ui.window(wid).await.ok_or(Error::WindowNotFound(wid))?;
    println!("{}", serde_json::to_string_pretty(&w.info)?);
    Ok(())
}

async fn del(mgr: Arc<SessionService>, id: &str) -> anyhow::Result<()> {
    let wid = WindowId::parse(id)?;
    mgr.del_window(wid).await?;
    println!("deleted window: {wid}");
    Ok(())
}

async fn add_pane(mgr: Arc<SessionService>, args: WindowPaneArgs) -> anyhow::Result<()> {
    let wid = WindowId::parse(&args.window)?;
    let pid = crate::ui::PaneId::parse(&args.pane)?;
    mgr.add_pane_to_window(wid, pid).await?;
    println!("added pane {pid} to window {wid}");
    Ok(())
}

async fn remove_pane(mgr: Arc<SessionService>, args: WindowPaneArgs) -> anyhow::Result<()> {
    let wid = WindowId::parse(&args.window)?;
    let pid = crate::ui::PaneId::parse(&args.pane)?;
    mgr.remove_pane_from_window(wid, pid).await?;
    println!("removed pane {pid} from window {wid}");
    Ok(())
}
