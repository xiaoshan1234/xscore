// cli/link.rs
use crate::core::{Error, LinkId, SessionManager};
use crate::url;
use clap::{Args, Subcommand};
use std::sync::Arc;

#[derive(Subcommand)]
pub enum LinkCmd {
    /// Create a new link + first pane (URL or `-c CONFIG` is required)
    Add(LinkAddArgs),

    /// Attach to an existing tmux session by URL or `-c CONFIG` (tmux only)
    Open(LinkOpenArgs),

    /// Detach a tmux link from in-memory registry (tmux session keeps running; non-tmux errors)
    Close { #[arg(long)] id: String },

    /// Full teardown: kill transport + sink + every pane
    Del { #[arg(long)] id: String },

    /// Print LinkInfo JSON
    Show { #[arg(long)] id: String },

    /// List all in-memory links
    List,

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
}

#[derive(Args)]
pub struct LinkOpenArgs {
    /// URL of the existing tmux session to attach (or omit if `-c CONFIG` is provided)
    pub url: Option<String>,

    /// Use a saved config template (resolves through `link config show <name>`)
    #[arg(short = 'c', long)]
    pub config: Option<String>,
}

#[derive(Subcommand)]
pub enum LinkConfigCmd {
    /// Save a ConnectSpec template (does not start the link)
    Add {
        /// Name for the template (also the file basename under `links/`)
        name: String,
        /// URL to remember
        url: String,
    },
    /// Delete a template file
    Remove { name: String },
    /// Print a template
    Show { name: String },
    /// List all templates
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
        LinkCmd::Config { cmd } => config_cmd(cmd).await,
    }
}

async fn add(mgr: Arc<SessionManager>, args: LinkAddArgs) -> anyhow::Result<()> {
    // Resolve to (spec, address): either via URL, via saved template, or URL+template merge.
    let (spec, address) = resolve_link_input(args.url.as_deref(), args.config.as_deref())?;
    let (link_id, window_id, pane_id) = mgr.add_link(spec, address).await?;
    // Print all returned IDs so users (and tests) can chain commands.
    // For raw links (the new default), no window/pane — only `link:` is printed.
    // For tmux links, all three IDs are printed (one default window + one default pane).
    println!("link:  {link_id}");
    if let Some(wid) = window_id {
        println!("window:{wid}");
    }
    if let Some(pid) = pane_id {
        println!("pane:  {pid}");
    }
    Ok(())
}

async fn open(mgr: Arc<SessionManager>, args: LinkOpenArgs) -> anyhow::Result<()> {
    // Same input resolution as `add`. The manager rejects non-tmux with `OpenOnlyForTmux`.
    let (spec, address) = resolve_link_input(args.url.as_deref(), args.config.as_deref())?;
    let (link_id, _wid, pane_id) = mgr.open_link(spec, address).await?;
    println!("link:  {link_id}");
    if let Some(pid) = pane_id {
        println!("pane:  {pid}");
    }
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
    println!("{}", serde_json::to_string_pretty(&link.info)?);
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
            let spec = url::parse(&url)?.0;
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

/// Resolve `(url?, -c CONFIG?)` to `(ConnectSpec, LinkAddress)`. Used by
/// both `link add` and `link open`. At least one input is required.
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
            // The saved template already encodes the transport tuple and sink
            // — its `address` is what was registered when the template was made.
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

/// `link add -c <name> <url>` means: take the saved template (with its
/// transport + sink) and override the transport with the CLI URL.
fn merge_specs(template: crate::model::ConnectSpec, cli: crate::model::ConnectSpec) -> crate::model::ConnectSpec {
    crate::model::ConnectSpec {
        name: if cli.name.is_empty() { template.name } else { cli.name },
        transport: cli.transport,
        sink: template.sink,
        address: cli.address,
    }
}