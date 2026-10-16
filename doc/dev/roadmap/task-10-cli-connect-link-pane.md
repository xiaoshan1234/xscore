# Task 10: CLI — `connect`, `link`, `pane` subcommands

**Objective:** Wire `connect <url> [--save] [--link=existing]`, `link list/show/close/save/load`, `pane list/show/close`.

**Files:**
- Create: `crates/xscore/src/cli/{mod,connect,link,pane}.rs`
- Modify: `crates/xscore/src/main.rs`

**Step 1: cli/mod.rs**

```rust
// cli/mod.rs
pub mod config;
pub mod connect;
pub mod data;
pub mod link;
pub mod pane;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "xscore", version, about = "Manage SSH / shell / serial sessions as Link x Pane")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Subcommand)]
pub enum Commands {
    /// Open a session from a URL (creates or attaches to a link, returns a pane id)
    Connect(connect::ConnectArgs),
    /// Inspect / close a link (the physical connection)
    Link(link::LinkCmd),
    /// Inspect / close a pane (one view into a link)
    Pane(pane::PaneCmd),
    /// Read / write / resize data on a pane
    Data(data::DataCmd),
    /// Show / edit config values
    Config(config::ConfigCmd),
}
```

**Step 2: cli/connect.rs**

```rust
// cli/connect.rs
use clap::Args;
use std::sync::Arc;

#[derive(Args)]
pub struct ConnectArgs {
    /// URL: shell://, ssh://, serial://
    pub url: String,

    /// Save the parsed spec to ~/.config/xscore/sessions/link-<id>.json
    #[arg(long)]
    pub save: bool,

    /// Attach a new pane to an existing link matching the address tuple.
    #[arg(long)]
    pub link_existing: bool,
}

pub async fn run(mgr: Arc<SessionManager>, args: ConnectArgs) -> anyhow::Result<()> {
    let (spec, address) = xscore::url::parse(&args.url)?;
    let (link_id, pane_id) = mgr.connect(spec.clone(), address, args.link_existing).await?;
    eprintln!("link:  {link_id}");
    println!("pane:  {pane_id}");
    if args.save {
        xscore::config::ensure_dirs()?;
        xscore::config::save_connect_spec(
            &xscore::config::link_path(&link_id.to_string()), &spec)?;
    }
    Ok(())
}
```

**Step 3: cli/link.rs**

```rust
// cli/link.rs
use crate::core::LinkId;
use crate::core::SessionManager;
use clap::Subcommand;
use std::sync::Arc;

#[derive(Subcommand)]
pub enum LinkCmd {
    List,
    Show { #[arg(long)] id: String },
    Close { #[arg(long)] id: String },
    Save { #[arg(long)] id: String },
    Load { #[arg(long)] path: String },
}

pub async fn run(mgr: Arc<SessionManager>, cmd: LinkCmd) -> anyhow::Result<()> {
    match cmd {
        LinkCmd::List => {
            for link in mgr.links.list().await {
                let info = &link.info;
                println!("{}\t{}\t{}", info.id, info.transport_str(), info.name);
            }
        }
        LinkCmd::Show { id } => {
            let lid = LinkId::parse(&id)?;
            let link = mgr.links.get(lid).await.ok_or(crate::core::Error::LinkNotFound(lid))?;
            println!("{}", serde_json::to_string_pretty(&link.info)?);
        }
        LinkCmd::Close { id } => {
            let lid = LinkId::parse(&id)?;
            mgr.close_link(lid).await?;
            println!("closed link: {lid}");
        }
        LinkCmd::Save { id } => {
            let lid = LinkId::parse(&id)?;
            let link = mgr.links.get(lid).await.ok_or(crate::core::Error::LinkNotFound(lid))?;
            let spec = link_to_spec(&link.info);
            xscore::config::ensure_dirs()?;
            xscore::config::save_connect_spec(
                &xscore::config::link_path(&lid.to_string()), &spec)?;
            println!("saved: {lid}");
        }
        LinkCmd::Load { path } => {
            let spec = xscore::config::load_connect_spec(std::path::Path::new(&path))?;
            println!("{}", serde_json::to_string_pretty(&spec)?);
        }
    }
    Ok(())
}

fn link_to_spec(info: &crate::model::LinkInfo) -> xscore::model::ConnectSpec {
    xscore::model::ConnectSpec {
        name: info.name.clone(),
        transport: info.transport.clone(),
        sink: info.sink.clone(),
    }
}
```

**Step 4: cli/pane.rs**

```rust
// cli/pane.rs
use crate::core::PaneId;
use crate::core::SessionManager;
use clap::Subcommand;
use std::sync::Arc;

#[derive(Subcommand)]
pub enum PaneCmd {
    /// List panes (optionally filtered by --link <link-id>)
    List { #[arg(long)] link: Option<String> },
    Show { #[arg(long)] id: String },
    Close { #[arg(long)] id: String },
}

pub async fn run(mgr: Arc<SessionManager>, cmd: PaneCmd) -> anyhow::Result<()> {
    match cmd {
        PaneCmd::List { link } => {
            let all = mgr.panes.list_all().await;
            for p in all {
                if let Some(lid) = &link {
                    if p.link_id.to_string() != *lid { continue; }
                }
                println!("{}\t{}\t{}", p.id, p.link_id, p.sink_handle);
            }
        }
        PaneCmd::Show { id } => {
            let pid = PaneId::parse(&id)?;
            let info = mgr.panes.get(pid).await.ok_or(crate::core::Error::PaneNotFound(pid))?;
            println!("{}", serde_json::to_string_pretty(&info)?);
        }
        PaneCmd::Close { id } => {
            let pid = PaneId::parse(&id)?;
            mgr.close_pane(pid).await?;
            println!("closed pane: {pid}");
        }
    }
    Ok(())
}
```

**Step 5: main.rs**

```rust
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();
    let cli = xscore::cli::Cli::parse();
    let mgr = std::sync::Arc::new(xscore::core::SessionManager::new());
    match cli.command {
        xscore::cli::Commands::Connect(c) => xscore::cli::connect::run(mgr, c).await?,
        xscore::cli::Commands::Link(c) => xscore::cli::link::run(mgr, c).await?,
        xscore::cli::Commands::Pane(c) => xscore::cli::pane::run(mgr, c).await?,
        xscore::cli::Commands::Data(_c) => println!("data (todo)"),
        xscore::cli::Commands::Config(_c) => println!("config (todo)"),
    }
    Ok(())
}
```

**Step 6: Smoke**

```bash
cargo run -p xscore -- connect 'shell:///bin/cat' --save
cargo run -p xscore -- link list
cargo run -p xscore -- pane list
cargo run -p xscore -- pane close --id <pane-id>     # closes pane, link stays
cargo run -p xscore -- link close --id <link-id>     # closes the connection
```

**Step 7: Commit**

```bash
git add crates/xscore/src/cli/ crates/xscore/src/main.rs
git commit -m "feat(cli): connect/link/pane subcommands over Link x Pane"
```