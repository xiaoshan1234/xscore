// app/cli/link.rs (v0.6.0 — a link is one connection inside a session)
//
// `link add` creates a `Pipe` link (a transport xscore owns) plus its first
// window and pane; a tmux session's single link is created by `session new
// --tmux` / `session attach` and cannot be added to or deleted here (ADR 0010
// §4/§D5). The old open/close/reconcile verbs are gone with the old tmux path:
// those jobs belong to the session verbs and to control-mode events.
use crate::service::link_service::LinkId;
use crate::service::session_service::SessionService;
use crate::ui::PaneId;
use crate::Error;

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
    /// is refused inside one; use `session new --tmux` / `session attach`.
    Add(LinkAddArgs),

    /// Full teardown of a pipe link: close its transport and orphan every
    /// attached pane (buffers kept).
    Del {
        #[arg(long)]
        id: String,
    },

    /// Print LinkInfo JSON
    Show {
        #[arg(long)]
        id: String,
    },

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

    /// Point an existing UI pane at the Buffer produced by the new Link.
    #[arg(long, value_name = "PANE_ID")]
    pub pane: Option<String>,

    /// Session that owns the link and receives its buffer.
    #[arg(long, value_name = "NAME")]
    pub session: String,
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

pub async fn run(mgr: Arc<SessionService>, cmd: LinkCmd) -> anyhow::Result<()> {
    match cmd {
        LinkCmd::Add(args) => add(mgr, args).await,
        LinkCmd::Del { id } => del(mgr, &id).await,
        LinkCmd::Show { id } => show(mgr, &id).await,
        LinkCmd::List(args) => list(mgr, args).await,
        LinkCmd::Config { cmd } => config_cmd(cmd).await,
    }
}

async fn add(mgr: Arc<SessionService>, args: LinkAddArgs) -> anyhow::Result<()> {
    let (spec, address, pane_params) =
        resolve_link_input(args.url.as_deref(), args.config.as_deref())?;
    let supplied_id = match args.id.as_deref() {
        Some(s) => Some(LinkId::parse(s)?),
        None => None,
    };
    let pane = match args.pane.as_deref() {
        Some(p) => Some(PaneId::parse(p)?),
        None => None,
    };
    let sid = mgr.resolve_session(Some(&args.session)).await?;
    let result = mgr
        .add_link_in(sid, spec, address, supplied_id, pane)
        .await?;
    println!("link:  {}", result.link_id);
    println!("buffer: {}", result.buffer_id);
    if let Some(wid) = result.window_id {
        println!("window: {wid}");
    }
    println!("pane:  {}", result.pane_id);
    // Geometry rides in the Link URL (`?cols=120&rows=40`).
    if pane_params.cols.is_some() || pane_params.rows.is_some() {
        mgr.resize_pane(
            result.pane_id,
            pane_params.cols.unwrap_or(80),
            pane_params.rows.unwrap_or(24),
        )
        .await?;
    }
    Ok(())
}

async fn del(mgr: Arc<SessionService>, id: &str) -> anyhow::Result<()> {
    let lid = LinkId::parse(id)?;
    mgr.del_link(lid).await?;
    println!("deleted: {lid}");
    Ok(())
}

async fn show(mgr: Arc<SessionService>, id: &str) -> anyhow::Result<()> {
    let lid = LinkId::parse(id)?;
    let link = mgr
        .link_service
        .get(lid)
        .await
        .ok_or(Error::LinkNotFound(lid))?;
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

async fn list(mgr: Arc<SessionService>, args: LinkListArgs) -> anyhow::Result<()> {
    let only = match args.session.as_deref() {
        Some(name) => Some(mgr.resolve_session(Some(name)).await?),
        None => None,
    };
    println!("{:<36}  {:<36}  {:<16}", "LINK ID", "NAME", "KIND");
    for link in mgr.link_service.list().await {
        let info = &link.info;
        if let Some(sid) = only {
            if info.session_id != sid {
                continue;
            }
        }
        // `control/shell` vs `control/ssh` matters: it is how a tmux session
        // says whether its server is local or reached over ssh.
        let kind = if info.is_control() { "control" } else { "pipe" };
        println!(
            "{:<36}  {:<36}  {:<16}",
            info.id,
            info.name,
            format!("{kind}/{}", info.address.kind())
        );
    }
    Ok(())
}

async fn config_cmd(cmd: LinkConfigCmd) -> anyhow::Result<()> {
    match cmd {
        LinkConfigCmd::Add { name, url } => {
            let spec = crate::infra::url::parse(&url)?.0;
            crate::infra::config::ensure_dirs()?;
            crate::infra::config::save_connect_spec(
                &crate::infra::config::link_config_path(&name),
                &spec,
            )?;
            println!("saved template: {name}");
        }
        LinkConfigCmd::Remove { name } => {
            let p = crate::infra::config::link_config_path(&name);
            if !p.exists() {
                anyhow::bail!("no such template: {name}");
            }
            std::fs::remove_file(p)?;
            println!("removed template: {name}");
        }
        LinkConfigCmd::Show { name } => {
            let spec = crate::infra::config::load_connect_spec(
                &crate::infra::config::link_config_path(&name),
            )?;
            println!("{}", serde_json::to_string_pretty(&spec)?);
        }
        LinkConfigCmd::List => {
            println!("{:<24}", "TEMPLATE");
            for p in crate::infra::config::list_link_configs()? {
                if let Some(s) = p.file_stem().and_then(|x| x.to_str()) {
                    println!("{:<24}", s);
                }
            }
        }
    }
    Ok(())
}

pub(crate) fn resolve_link_input(
    url: Option<&str>,
    config_name: Option<&str>,
) -> anyhow::Result<(
    crate::infra::link::ConnectSpec,
    crate::infra::link::LinkAddress,
    crate::infra::url::PaneParams,
)> {
    let parsed = match (url, config_name) {
        (None, None) => anyhow::bail!("provide either <url> or -c CONFIG"),
        (Some(u), None) => crate::infra::url::parse_with_pane(u)?,
        (None, Some(c)) => {
            let template_path = crate::infra::config::link_config_path(c);
            let spec = crate::infra::config::load_connect_spec(&template_path)?;
            let address = spec.address.clone();
            (spec, address, crate::infra::url::PaneParams::default())
        }
        (Some(u), Some(c)) => {
            let template_path = crate::infra::config::link_config_path(c);
            let base_spec = crate::infra::config::load_connect_spec(&template_path)?;
            let (cli_spec, cli_addr, pane) = crate::infra::url::parse_with_pane(u)?;
            (merge_specs(base_spec, cli_spec), cli_addr, pane)
        }
    };
    // A cwd that is not a directory is silently ignored by the pty layer (the
    // child just starts in $HOME), so check it before anything is spawned.
    if let crate::infra::link::TransportConfig::Shell { cwd: Some(dir), .. } = &parsed.0.transport {
        crate::infra::util::require_dir(dir)?;
    }
    if !cfg!(windows) {
        if let crate::infra::link::TransportConfig::Shell { command, .. } = &parsed.0.transport {
            if crate::infra::util::paths::looks_like_windows_program(command) {
                eprintln!(
                    "warning: `{command}` is a Windows program. WSL interop gives it no \
                     console behind a pty, so it typically produces no output; run it \
                     from inside a WSL shell pane instead."
                );
            }
        }
    }
    Ok(parsed)
}

fn merge_specs(
    template: crate::infra::link::ConnectSpec,
    cli: crate::infra::link::ConnectSpec,
) -> crate::infra::link::ConnectSpec {
    crate::infra::link::ConnectSpec {
        name: if cli.name.is_empty() {
            template.name
        } else {
            cli.name
        },
        transport: cli.transport,
        address: cli.address,
    }
}
