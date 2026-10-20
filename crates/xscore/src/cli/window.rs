// cli/window.rs (v0.4.0 — windows may be virtual or mirror a tmux window)
use crate::core::{Error, LinkId, SessionManager, WindowId};
use crate::core::window::Window;
use crate::model::WindowLayout;
use clap::{Args, Subcommand};
use std::str::FromStr;
use std::sync::Arc;

#[derive(Subcommand)]
pub enum WindowCmd {
    /// Create a window: virtual by default, or a real tmux window with --link.
    Add(WindowAddArgs),
    /// List all windows.
    List,
    /// Print WindowInfo JSON.
    Show { #[arg(long)] id: String },
    /// Tear down a window. Panes inside survive as orphans.
    Del { #[arg(long)] id: String },
    /// Update a window's layout hint (single|tabs|split_h|split_v|grid[<cols>,<rows>]).
    SetLayout(WindowSetLayoutArgs),
    /// Add a pane to this window.
    AddPane(WindowPaneArgs),
    /// Remove a pane from this window (pane stays alive as orphan).
    RemovePane(WindowPaneArgs),
}

#[derive(Args)]
pub struct WindowAddArgs {
    /// Window name (becomes `w-<first8 of window-id>` if absent).
    #[arg(long)]
    pub name: Option<String>,
    /// Layout shape: single (default) | tabs | split_h | split_v | grid.
    #[arg(long, default_value = "single")]
    pub layout: String,
    /// Total window width in cells.
    #[arg(long, default_value_t = 120)]
    pub cols: u16,
    /// Total window height in cells.
    #[arg(long, default_value_t = 30)]
    pub rows: u16,
    /// Create a real tmux window inside this tmux link. With --link the
    /// tmux-reported `@N` handle, layout and geometry WIN over
    /// --layout/--cols/--rows.
    #[arg(long, value_name = "LINK_ID")]
    pub link: Option<String>,
}

#[derive(Args)]
pub struct WindowSetLayoutArgs {
    #[arg(long)] pub id: String,
    /// New layout: single | tabs | split_h | split_v | grid[<cols>,<rows>].
    pub layout: String,
}

#[derive(Args)]
pub struct WindowPaneArgs {
    #[arg(long)] pub window: String,
    #[arg(long)] pub pane: String,
}

pub async fn run(mgr: Arc<SessionManager>, cmd: WindowCmd) -> anyhow::Result<()> {
    match cmd {
        WindowCmd::Add(args) => add(mgr, args).await,
        WindowCmd::List => list(mgr).await,
        WindowCmd::Show { id } => show(mgr, &id).await,
        WindowCmd::Del { id } => del(mgr, &id).await,
        WindowCmd::SetLayout(args) => set_layout(mgr, args).await,
        WindowCmd::AddPane(args) => add_pane(mgr, args).await,
        WindowCmd::RemovePane(args) => remove_pane(mgr, args).await,
    }
}

fn parse_layout(s: &str) -> anyhow::Result<WindowLayout> {
    // Accepts: single | tabs | split_h | split_v | grid
    //   For grid: "grid" (1x1 default) or "grid:3x2"` — colons are
    //   inconvenient on the shell, so use "grid[<cols>x<rows>]"
    //   or "grid<cols>x<rows>" — actually keep it simple:
    //   `grid` alone = 1x1, `grid-NxM` = NxM (dash separator avoids
    //   clap arg-value splitting issues).
    match s {
        "single" => Ok(WindowLayout::Single),
        "tabs" => Ok(WindowLayout::Tabs),
        "split_h" => Ok(WindowLayout::SplitHorizontal),
        "split_v" => Ok(WindowLayout::SplitVertical),
        "grid" => Ok(WindowLayout::Grid { cols: 1, rows: 1 }),
        grid if grid.starts_with("grid-") => {
            let dims = &grid[5..];
            let mut parts = dims.splitn(2, 'x');
            let cols: u16 = parts.next().unwrap_or("1").parse()
                .map_err(|_| anyhow::anyhow!("invalid grid dims: {s}"))?;
            let rows: u16 = parts.next().unwrap_or("1").parse()
                .map_err(|_| anyhow::anyhow!("invalid grid dims: {s}"))?;
            Ok(WindowLayout::Grid { cols, rows })
        }
        other => anyhow::bail!("unknown layout: {other} (expected single|tabs|split_h|split_v|grid[-NxM])"),
    }
}

async fn add(mgr: Arc<SessionManager>, args: WindowAddArgs) -> anyhow::Result<()> {
    let layout = parse_layout(&args.layout)?;
    let link = match args.link.as_deref() {
        Some(s) => Some(LinkId::parse(s)?),
        None => None,
    };
    let wid = mgr.add_window(args.name, layout, args.cols, args.rows, link).await?;
    println!("window: {wid}");
    Ok(())
}

async fn list(mgr: Arc<SessionManager>) -> anyhow::Result<()> {
    println!(
        "{:<36}  {:<24}  {:<10}  {:<5}  {:<5}  {:<8}",
        "WINDOW ID", "NAME", "LAYOUT", "COLS", "ROWS", "TMUX"
    );
    let mut all: Vec<Arc<Window>> = mgr.windows.list_all().await;
    // Stable order by id (uuid v7 is chronological).
    all.sort_by_key(|p| p.info.id.to_string());
    for w in all {
        let layout = match &w.info.layout {
            WindowLayout::Single => "single".to_string(),
            WindowLayout::Tabs => "tabs".to_string(),
            WindowLayout::SplitHorizontal => "split_h".to_string(),
            WindowLayout::SplitVertical => "split_v".to_string(),
            WindowLayout::Grid { cols, rows } => format!("grid-{cols}x{rows}"),
        };
        let tmux = w.info.tmux_window_handle.clone().unwrap_or_else(|| "-".into());
        println!(
            "{:<36}  {:<24}  {:<10}  {:<5}  {:<5}  {:<8}",
            w.info.id, w.info.name, layout, w.info.cols, w.info.rows, tmux
        );
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

async fn set_layout(mgr: Arc<SessionManager>, args: WindowSetLayoutArgs) -> anyhow::Result<()> {
    let wid = WindowId::parse(&args.id)?;
    let layout = parse_layout(&args.layout)?;
    mgr.set_window_layout(wid, layout).await?;
    println!("layout: {} ({})", args.layout, wid);
    Ok(())
}

async fn add_pane(mgr: Arc<SessionManager>, args: WindowPaneArgs) -> anyhow::Result<()> {
    let wid = WindowId::parse(&args.window)?;
    let pid = crate::core::PaneId::parse(&args.pane)?;
    mgr.add_pane_to_window(wid, pid).await?;
    println!("added pane {pid} to window {wid}");
    Ok(())
}

async fn remove_pane(mgr: Arc<SessionManager>, args: WindowPaneArgs) -> anyhow::Result<()> {
    let wid = WindowId::parse(&args.window)?;
    let pid = crate::core::PaneId::parse(&args.pane)?;
    mgr.remove_pane_from_window(wid, pid).await?;
    println!("removed pane {pid} from window {wid}");
    Ok(())
}

// silence unused-import lint for FromStr
#[allow(dead_code)]
fn _unused_fromstr<T: FromStr>(_: T) {}