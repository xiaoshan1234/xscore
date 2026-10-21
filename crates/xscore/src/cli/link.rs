// cli/link.rs (v0.6.0 — a link is one connection inside a session)
//
// `link add` creates a `Pipe` link (a transport xscore owns) plus its first
// window and pane; a tmux session's single link is created by `session new
// --tmux` / `session adopt` and cannot be added to or deleted here (ADR 0010
// §4/§D5). The old open/close/reconcile verbs are gone with the old tmux path:
// those jobs belong to the session verbs and to control-mode events.
use crate::core::{Error, LinkId, PaneId, SessionManager};
use crate::url;
use clap::{Args, Subcommand};
use std::sync::Arc;

#[derive(Subcommand)]
pub enum LinkCmd {
    /// Create a pipe link plus its first window/pane.
    ///
    /// * default: a native window + a pane with a ring buffer
    /// * `--pane <pid>`: bind that existing pane instead
    ///
    /// A tmux session has exactly one link — its control client — so this verb
    /// is refused inside one; use `session new --tmux` / `session adopt`.
    Add(LinkAddArgs),

    /// Full teardown of a pipe link: close its transport and orphan every
    /// attached pane (buffers kept).
    Del { #[arg(long)] id: String },

    /// Print LinkInfo JSON
    Show { #[arg(long)] id: String },

    /// List links, optionally only one session's.
    List(LinkListArgs),

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
    /// Refused for a pane that is already attached (detach it first) and for a
    /// pane tmux owns (its `%N` addresses it on its session's control link).
    #[arg(long, value_name = "PANE_ID")]
    pub pane: Option<String>,

    /// Put the link in this session. Omitted: the manager resolves the default
    /// (the only session, else `default`, created on demand).
    #[arg(long, value_name = "NAME")]
    pub session: Option<String>,
}

#[derive(Args)]
pub struct LinkListArgs {
    /// Only this session's links.
    #[arg(long, value_name = "NAME")]
    pub session: Option<String>,
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
        LinkCmd::Del { id } => del(mgr, &id).await,
        LinkCmd::Show { id } => show(mgr, &id).await,
        LinkCmd::List(args) => list(mgr, args).await,
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

async fn list(mgr: Arc<SessionManager>, args: LinkListArgs) -> anyhow::Result<()> {
    let only = match args.session.as_deref() {
        Some(name) => Some(mgr.resolve_session(Some(name)).await?),
        None => None,
    };
    println!("{:<36}  {:<36}  {:<8}", "LINK ID", "NAME", "KIND");
    for link in mgr.links.list().await {
        let info = &link.info;
        if let Some(sid) = only {
            if info.session_id != sid {
                continue;
            }
        }
        let kind = if info.is_control() { "control" } else { "pipe" };
        println!("{:<36}  {:<36}  {:<8}", info.id, info.name, kind);
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
        address: cli.address,
    }
}