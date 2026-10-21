// cli/session.rs (v0.6.0 — the Session tier, ADR 0010 §1, §10)
//
// `xscore session {ls,new,show,kill,forget,rename}`: the workspace verbs.
//
// `session new` is the one-command path: a native session immediately gets a
// first link, window and pane so the common case is a single invocation
// (arch.md §10). `--empty` skips that for a bare workspace.
//
// `kill` destroys the backend and the rows; `forget` only drops the rows and
// leaves tmux running. For a native session the two cannot differ — the daemon
// IS the backend — so `forget` refuses.
use crate::core::{Error, LinkAddress, SessionId, SessionManager};
use crate::model::{
    ConnectSpec, SessionBackend, SessionInfo, SinkConfig, TmuxHostConfig, TmuxSocket,
    TransportConfig,
};
use clap::{Args, Subcommand};
use std::sync::Arc;

#[derive(Subcommand)]
pub enum SessionCmd {
    /// List sessions with the number of windows, panes and links each owns.
    Ls,

    /// Create a native session. Unless --empty, it also gets its first
    /// link, window and pane, so the usual case is one command.
    New(SessionNewArgs),

    /// Print a session and the ids of its windows, panes and links.
    Show { name: String },

    /// Destroy a session and everything in it (the only cascade in the model).
    Kill { name: String },

    /// Drop xscore's rows for a tmux session and leave tmux running.
    Forget { name: String },

    /// Rename a session.
    Rename { old: String, new: String },
}

#[derive(Args)]
pub struct SessionNewArgs {
    /// Session name — the handle every other verb resolves.
    pub name: String,

    /// Create a bare workspace: no link, no window, no pane.
    #[arg(long)]
    pub empty: bool,

    /// Command for the first pane (defaults to $SHELL, then /bin/sh).
    #[arg(long, value_name = "CMD")]
    pub cmd: Option<String>,

    /// Working directory for the first pane's shell.
    #[arg(long, value_name = "DIR")]
    pub cwd: Option<String>,

    /// First pane width in cells (default 80).
    #[arg(long)]
    pub cols: Option<u16>,

    /// First pane height in cells (default 24).
    #[arg(long)]
    pub rows: Option<u16>,
}

pub async fn run(mgr: Arc<SessionManager>, cmd: SessionCmd) -> anyhow::Result<()> {
    match cmd {
        SessionCmd::Ls => ls(mgr).await,
        SessionCmd::New(args) => new(mgr, args).await,
        SessionCmd::Show { name } => show(mgr, &name).await,
        SessionCmd::Kill { name } => kill(mgr, &name).await,
        SessionCmd::Forget { name } => forget(mgr, &name).await,
        SessionCmd::Rename { old, new } => rename(mgr, &old, &new).await,
    }
}

async fn ls(mgr: Arc<SessionManager>) -> anyhow::Result<()> {
    println!(
        "{:<36}  {:<24}  {:<24}  {:<7}  {:<5}  {:<5}",
        "SESSION ID", "NAME", "BACKEND", "WINDOWS", "PANES", "LINKS"
    );
    let windows = mgr.windows.list_all().await;
    let panes = mgr.panes.list_all().await;
    let links = mgr.links.list().await;
    for info in mgr.list_sessions().await? {
        let w = windows.iter().filter(|x| x.info.session_id == info.id).count();
        let p = panes.iter().filter(|x| x.session_id == info.id).count();
        let l = links.iter().filter(|x| x.info.session_id == info.id).count();
        println!(
            "{:<36}  {:<24}  {:<24}  {:<7}  {:<5}  {:<5}",
            info.id,
            info.name,
            backend_label(&info),
            w,
            p,
            l
        );
    }
    Ok(())
}

async fn new(mgr: Arc<SessionManager>, args: SessionNewArgs) -> anyhow::Result<()> {
    let sid = mgr.add_session_native(&args.name).await?;
    println!("session: {sid}");
    if args.empty {
        return Ok(());
    }

    // arch.md §10: --cmd/--cwd belong here because this verb starts a process;
    // a URL only says how to connect.
    let command = args
        .cmd
        .clone()
        .or_else(|| std::env::var("SHELL").ok())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "/bin/sh".into());
    let address = LinkAddress::Shell { command: command.clone(), args: vec![] };
    let spec = ConnectSpec {
        // Empty name: the manager derives one from the new link id.
        name: String::new(),
        transport: TransportConfig::Shell { command, args: vec![], cwd: args.cwd.clone() },
        sink: SinkConfig::Raw,
        address: address.clone(),
    };
    let result = mgr.add_link_in(sid, spec, address, None, None).await?;
    println!("link:   {}", result.link_id);
    if let Some(wid) = result.window_id {
        println!("window: {wid}");
    }
    println!("pane:   {}", result.pane_id);

    // Only when asked: a resize of a fresh pty is harmless but pointless, and
    // on the default path it must not be able to fail the command.
    if args.cols.is_some() || args.rows.is_some() {
        mgr.resize_pane(result.pane_id, args.cols.unwrap_or(80), args.rows.unwrap_or(24))
            .await?;
    }
    Ok(())
}

async fn show(mgr: Arc<SessionManager>, name: &str) -> anyhow::Result<()> {
    let sid = resolve(&mgr, name).await?;
    let info = mgr.get_session(sid).await?;
    println!("session: {}", info.id);
    println!("name:    {}", info.name);
    println!("backend: {}", backend_label(&info));
    println!("created: {}", info.created_at.to_rfc3339());

    let mut links: Vec<_> = mgr
        .links
        .list()
        .await
        .into_iter()
        .filter(|l| l.info.session_id == sid)
        .collect();
    links.sort_by_key(|l| l.info.created_at);
    for l in links {
        println!("link:    {}", l.info.id);
    }

    let mut windows = mgr.windows.list_all().await;
    windows.retain(|w| w.info.session_id == sid);
    windows.sort_by_key(|w| w.info.created_at);
    for w in windows {
        println!("window:  {}", w.info.id);
    }

    let mut panes = mgr.panes.list_all().await;
    panes.retain(|p| p.session_id == sid);
    panes.sort_by_key(|p| p.created_at);
    for p in panes {
        println!("pane:    {}", p.id);
    }
    Ok(())
}

async fn kill(mgr: Arc<SessionManager>, name: &str) -> anyhow::Result<()> {
    let sid = resolve(&mgr, name).await?;
    mgr.del_session(sid).await?;
    println!("killed session: {sid}");
    Ok(())
}

async fn forget(mgr: Arc<SessionManager>, name: &str) -> anyhow::Result<()> {
    let sid = resolve(&mgr, name).await?;
    mgr.forget_session(sid).await?;
    println!("forgot session: {sid}");
    Ok(())
}

async fn rename(mgr: Arc<SessionManager>, old: &str, new: &str) -> anyhow::Result<()> {
    let sid = resolve(&mgr, old).await?;
    mgr.rename_session(sid, new).await?;
    println!("renamed: {old} -> {new}");
    Ok(())
}

/// Resolve a `<ref>` to a session id. Named sessions are looked up by name and
/// never fall back to the default: a typo must fail, not write elsewhere.
async fn resolve(mgr: &SessionManager, name: &str) -> anyhow::Result<SessionId> {
    mgr.sessions
        .get_by_name(name)
        .await
        .map(|s| s.info.id)
        .ok_or_else(|| Error::SessionNotFound(name.to_string()).into())
}

fn backend_label(info: &SessionInfo) -> String {
    match &info.backend {
        SessionBackend::Native => "native".to_string(),
        SessionBackend::Tmux { host, socket, session } => {
            let host = match host {
                TmuxHostConfig::Local => "local".to_string(),
                TmuxHostConfig::Ssh { destination, .. } => destination.clone(),
            };
            let socket = match socket {
                TmuxSocket::Default => "default".to_string(),
                TmuxSocket::Named(n) => n.clone(),
                TmuxSocket::Path(p) => p.clone(),
            };
            format!("tmux {host}/{socket}/{session}")
        }
    }
}
