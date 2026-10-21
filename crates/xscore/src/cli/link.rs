// cli/link.rs (v0.4.0 — `link add` auto-creates window/pane; tmux adoption)
use crate::core::{Error, LinkId, PaneId, SessionManager};
use crate::url;
use clap::{Args, Subcommand};
use std::sync::Arc;

#[derive(Subcommand)]
pub enum LinkCmd {
    /// Create a link plus its first window/pane.
    ///
    /// * default: a virtual window + a pane with a ring buffer
    /// * `?tmux=on`: adopt the tmux session's window/pane (tmux is the
    ///   source of truth for `@N`/`%N` and geometry)
    /// * `--pane <pid>`: bind that existing pane instead (direct links only)
    Add(LinkAddArgs),

    /// Attach to an existing tmux session by URL or `-c CONFIG` (tmux only)
    Open(LinkOpenArgs),

    /// No-op for tmux sessions (the tmux session is inherently persistent);
    /// errors for non-tmux links (ADR 0005 invariant).
    Close { #[arg(long)] id: String },

    /// Full teardown: kill transport + sink; detach every attached pane (silently).
    Del { #[arg(long)] id: String },

    /// Print LinkInfo JSON
    Show { #[arg(long)] id: String },

    /// List all in-memory links
    List,

    /// Compare xscore's registry against tmux's real structure (`@N`/`%N`).
    /// Read-only unless `--heal`; externally created panes are only reported.
    Sync {
        #[arg(long)] id: String,
        #[arg(long)] heal: bool,
    },

    /// Persistent ConnectSpec templates
    Config {
        #[command(subcommand)]
        cmd: LinkConfigCmd,
    },
}

#[derive(Args)]
pub struct LinkAddArgs {
    /// URL: shell://, ssh://, serial://, or omit if `-c CONFIG` is provided
    pub url: Option<String>,

    /// Use a saved config template (resolves through `link config show <name>`)
    #[arg(short = 'c', long)]
    pub config: Option<String>,

    /// Use a specific link id (uuid v7) instead of auto-generating one.
    /// Errors with `LinkIdExists` if the id is already in use.
    #[arg(long, value_name = "UUID")]
    pub id: Option<String>,

    /// Bind an EXISTING pane to the new link instead of creating one.
    /// Not allowed for tmux links (tmux owns its panes) and not allowed
    /// for a pane that is already attached (detach it first).
    #[arg(long, value_name = "PANE_ID")]
    pub pane: Option<String>,

    /// Put the link in this session. Omitted: the manager resolves the default
    /// (the only session, else `default`, created on demand).
    #[arg(long, value_name = "NAME")]
    pub session: Option<String>,
}

#[derive(Args)]
pub struct LinkOpenArgs {
    pub url: Option<String>,
    #[arg(short = 'c', long)]
    pub config: Option<String>,
}

#[derive(Subcommand)]
pub enum LinkConfigCmd {
    Add { name: String, url: String },
    Remove { name: String },
    Show { name: String },
    List,
}

pub async fn run(mgr: Arc<SessionManager>, cmd: LinkCmd) -> anyhow::Result<()> {
    match cmd {
        LinkCmd::Add(args) => add(mgr, args).await,
        LinkCmd::Open(args) => open(mgr, args).await,
        LinkCmd::Close { id } => close(mgr, &id).await,
        LinkCmd::Del { id } => del(mgr, &id).await,
        LinkCmd::Show { id } => show(mgr, &id).await,
        LinkCmd::List => list(mgr).await,
        LinkCmd::Sync { id, heal } => sync(mgr, &id, heal).await,
        LinkCmd::Config { cmd } => config_cmd(cmd).await,
    }
}

async fn add(mgr: Arc<SessionManager>, args: LinkAddArgs) -> anyhow::Result<()> {
    let (spec, address) = resolve_link_input(args.url.as_deref(), args.config.as_deref())?;
    let supplied_id = match args.id.as_deref() {
        Some(s) => Some(LinkId::parse(s)?),
        None => None,
    };
    let pane = match args.pane.as_deref() {
        Some(p) => Some(PaneId::parse(p)?),
        None => None,
    };
    let sid = mgr.resolve_session(args.session.as_deref()).await?;
    let result = mgr.add_link_in(sid, spec, address, supplied_id, pane).await?;
    println!("link:  {}", result.link_id);
    if let Some(wid) = result.window_id {
        println!("window: {wid}");
    }
    println!("pane:  {}", result.pane_id);
    Ok(())
}

async fn sync(mgr: Arc<SessionManager>, id: &str, heal: bool) -> anyhow::Result<()> {
    let lid = LinkId::parse(id)?;
    let report = mgr.sync_link(lid, heal).await?;
    println!("link: {}", lid);
    if report.is_clean() {
        println!("in sync: registry matches tmux");
        return Ok(());
    }
    for h in &report.missing_windows {
        println!("drift: window {h} exists in tmux but not in xscore");
    }
    for h in &report.stale_windows {
        println!("drift: window {h} in xscore is gone from tmux");
    }
    for h in &report.missing_panes {
        println!("drift: pane {h} exists in tmux but not in xscore (not adopted)");
    }
    for h in &report.stale_panes {
        println!("drift: pane {h} in xscore is gone from tmux");
    }
    if heal {
        println!(
            "healed: {} row(s) corrected, {} removed",
            report.healed, report.removed
        );
    } else {
        println!("run `xscore link sync --id {lid} --heal` to apply");
    }
    Ok(())
}

async fn open(mgr: Arc<SessionManager>, args: LinkOpenArgs) -> anyhow::Result<()> {
    let (spec, address) = resolve_link_input(args.url.as_deref(), args.config.as_deref())?;
    let link_id = mgr.open_link(spec, address).await?;
    println!("link:  {link_id}");
    Ok(())
}

async fn close(mgr: Arc<SessionManager>, id: &str) -> anyhow::Result<()> {
    let lid = LinkId::parse(id)?;
    mgr.close_link(lid).await?;
    println!("closed: {lid}");
    Ok(())
}

async fn del(mgr: Arc<SessionManager>, id: &str) -> anyhow::Result<()> {
    let lid = LinkId::parse(id)?;
    mgr.del_link(lid).await?;
    println!("deleted: {lid}");
    Ok(())
}

async fn show(mgr: Arc<SessionManager>, id: &str) -> anyhow::Result<()> {
    let lid = LinkId::parse(id)?;
    let link = mgr.links.get(lid).await.ok_or(Error::LinkNotFound(lid))?;
    // v0.4.1 (P0-3): surface `connected` alongside the persisted
    // LinkInfo. Since v0.5.0 (ADR 0009) a direct link's connection lives
    // in the resident daemon, so this is a LIVE probe, not a flag baked
    // in at hydration: `daemon stop` makes it false, re-attaching makes
    // it true again.
    let connected = mgr.link_is_connected(lid).await;
    let mut v = serde_json::to_value(&link.info)?;
    if let Some(obj) = v.as_object_mut() {
        obj.insert("connected".into(), serde_json::Value::Bool(connected));
    }
    println!("{}", serde_json::to_string_pretty(&v)?);
    Ok(())
}

async fn list(mgr: Arc<SessionManager>) -> anyhow::Result<()> {
    println!("{:<36}  {:<36}", "LINK ID", "NAME");
    for link in mgr.links.list().await {
        let info = &link.info;
        println!("{:<36}  {:<36}", info.id, info.name);
    }
    Ok(())
}

async fn config_cmd(cmd: LinkConfigCmd) -> anyhow::Result<()> {
    match cmd {
        LinkConfigCmd::Add { name, url } => {
            let spec = crate::url::parse(&url)?.0;
            crate::config::ensure_dirs()?;
            crate::config::save_connect_spec(
                &crate::config::link_config_path(&name), &spec)?;
            println!("saved template: {name}");
        }
        LinkConfigCmd::Remove { name } => {
            let p = crate::config::link_config_path(&name);
            if !p.exists() { anyhow::bail!("no such template: {name}"); }
            std::fs::remove_file(p)?;
            println!("removed template: {name}");
        }
        LinkConfigCmd::Show { name } => {
            let spec = crate::config::load_connect_spec(&crate::config::link_config_path(&name))?;
            println!("{}", serde_json::to_string_pretty(&spec)?);
        }
        LinkConfigCmd::List => {
            println!("{:<24}", "TEMPLATE");
            for p in crate::config::list_link_configs()? {
                if let Some(s) = p.file_stem().and_then(|x| x.to_str()) {
                    println!("{:<24}", s);
                }
            }
        }
    }
    Ok(())
}

fn resolve_link_input(
    url: Option<&str>,
    config_name: Option<&str>,
) -> anyhow::Result<(crate::model::ConnectSpec, crate::core::LinkAddress)> {
    match (url, config_name) {
        (None, None) => anyhow::bail!("provide either <url> or -c CONFIG"),
        (Some(u), None) => Ok(url::parse(u)?),
        (None, Some(c)) => {
            let template_path = crate::config::link_config_path(c);
            let spec = crate::config::load_connect_spec(&template_path)?;
            let address = spec.address.clone();
            Ok((spec, address))
        }
        (Some(u), Some(c)) => {
            let template_path = crate::config::link_config_path(c);
            let base_spec = crate::config::load_connect_spec(&template_path)?;
            let (cli_spec, cli_addr) = url::parse(u)?;
            Ok((merge_specs(base_spec, cli_spec), cli_addr))
        }
    }
}

fn merge_specs(template: crate::model::ConnectSpec, cli: crate::model::ConnectSpec) -> crate::model::ConnectSpec {
    crate::model::ConnectSpec {
        name: if cli.name.is_empty() { template.name } else { cli.name },
        transport: cli.transport,
        sink: template.sink,
        address: cli.address,
    }
}