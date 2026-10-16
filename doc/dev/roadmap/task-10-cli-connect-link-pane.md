# Task 10: CLI — `link`, `pane` subcommands

**Objective:** Wire the new two-tier CLI: `link {add,open,close,remove,show,list,config ...}` + `pane {add,open,close,remove,show,list}`.

**Background:** this is the revised CLI surface from the user-feedback round. The old `xscore connect <url>` is now `xscore link add <url>`. The old `xscore session {list,show,close,save,load}` and `xscore pane {list,show,close}` are renamed and extended. A new `link config {add,remove,show,list}` group manages persistent ConnectSpec templates under `~/.config/xscore/links/`.

**Files:**
- Create: `crates/xscore/src/cli/{mod,link,pane}.rs`
- Delete: `crates/xscore/src/cli/connect.rs` (replaced by `link::add`)
- Delete: `crates/xscore/src/cli/data.rs` (handled in Task 11)
- Modify: `crates/xscore/src/main.rs`
- Extend: `crates/xscore/src/core/manager.rs` (rename `connect` → `add_link` + add 4 new methods)
- Extend: `crates/xscore/src/core/link_registry.rs` (add `by_alias`)
- Extend: `crates/xscore/src/model/session.rs` (LinkInfo gains `alias`)
- Extend: `crates/xscore/src/config/{paths,loader}.rs` (new `links/` + `state/` paths)

---

## Command reference

```
xscore link add <url> [-c CONFIG] [-a ALIAS]     # create link + first pane
xscore link open <link-id>                       # re-attach existing link
xscore link close <link-id>                      # kill transport + sink + all panes
xscore link remove <link-id>                     # unregister only, leave tmux/server alive
xscore link show <link-id>
xscore link list

xscore link config add <alias> <url>             # save ConnectSpec template (no start)
xscore link config remove <alias>                # delete template
xscore link config show <alias>                  # read template
xscore link config list                          # list templates

xscore pane add <link-id> [--name=NAME]          # new_pane() inside the link
xscore pane open <pane-id> [--link=ID]           # re-attach to existing tmux window
xscore pane close <pane-id>                      # kill-pane + unregister
xscore pane remove <pane-id>                     # unregister only
xscore pane show <pane-id>
xscore pane list [--link LINK-ID]

xscore data read --pane <pane-id> [--bytes N]
xscore data write --pane <pane-id> [--from-file FILE]
xscore data resize --pane <pane-id> --cols C --rows R

xscore config show
xscore config path
xscore config set <key> <value>
xscore config unset <key>
```

### add vs open — create vs re-attach (semantics table)

| Verb | Creates a new external resource? | Touches in-memory registry? | Forgetting this leaks: |
| --- | --- | --- | --- |
| `link add`    | yes (transport + sink + 1 pane) | yes | processes / SSH channels / tmux sessions |
| `link open`   | no (re-attaches existing ones)  | yes | nothing — already running |
| `link close`  | no (kills them)                 | yes | nothing — already gone |
| `link remove` | no                              | yes | in-memory state only (tmux server stays) |
| `pane add`    | yes (one tmux window)           | yes | tmux window |
| `pane open`   | no                              | yes | nothing — window already exists |
| `pane close`  | no                              | yes | nothing — window already killed |
| `pane remove` | no                              | yes | in-memory state only (window stays) |

**Naming intent:** Linux file semantics — `add` creates, `remove` deletes the registration only, `close` releases the underlying resource, `open` connects/reconnects.

---

## Step 1: cli/mod.rs

```rust
// cli/mod.rs
pub mod config;
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
    /// Inspect / create / close / open / remove links (physical connections)
    Link(link::LinkCmd),
    /// Inspect / create / close / open / remove panes (views into links)
    Pane(pane::PaneCmd),
    /// Read / write / resize data on a pane
    Data(data::DataCmd),
    /// Show / edit AppConfig values
    Config(config::ConfigCmd),
}
```

---

## Step 2: cli/link.rs

```rust
// cli/link.rs
use crate::core::{Error, LinkId, SessionManager};
use crate::url;
use clap::{Args, Subcommand};
use std::sync::Arc;

#[derive(Subcommand)]
pub enum LinkCmd {
    /// Create a new link + first pane from a URL (or via a saved config template)
    Add(LinkAddArgs),

    /// Re-attach to an existing link (rebuild transport + sink; panes still register)
    Open { #[arg(long)] id: String },

    /// Kill transport + sink + every pane on this link
    Close { #[arg(long)] id: String },

    /// Unregister the link only. External resources (tmux / ssh / child) keep running.
    Remove { #[arg(long)] id: String },

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
    /// URL: shell://, ssh://, serial://
    pub url: String,

    /// Use a saved config template (resolves through `link config show <name>`)
    #[arg(short = 'c', long)]
    pub config: Option<String>,

    /// Runtime alias for this alive link (in-memory only, NOT persisted)
    #[arg(short = 'a', long)]
    pub alias: Option<String>,
}

#[derive(Subcommand)]
pub enum LinkConfigCmd {
    /// Save a ConnectSpec template (does not start the link)
    Add {
        /// Alias for the template
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
        LinkCmd::Open { id } => open(mgr, &id).await,
        LinkCmd::Close { id } => close(mgr, &id).await,
        LinkCmd::Remove { id } => remove(mgr, &id).await,
        LinkCmd::Show { id } => show(mgr, &id).await,
        LinkCmd::List => list(mgr).await,
        LinkCmd::Config { cmd } => config_cmd(cmd).await,
    }
}

async fn add(mgr: Arc<SessionManager>, args: LinkAddArgs) -> anyhow::Result<()> {
    // If -c is set, the user passed an alias + url. The template supplies
    // sink + name; the CLI URL supplies transport detail (port, identity, ...)
    // and LinkAddress (used for `--link=existing` later).
    let (spec, address) = if let Some(cname) = &args.config {
        let template_path = xscore::config::link_config_path(cname);
        let base_spec = xscore::config::load_connect_spec(&template_path)?;
        let (cli_spec, cli_addr) = url::parse(&args.url)?;
        (merge_specs(base_spec, cli_spec), cli_addr)
    } else {
        url::parse(&args.url)?
    };

    let (link_id, pane_id) = mgr.add_link(spec, address, args.alias.clone()).await?;
    eprintln!("link:  {link_id}");
    println!("pane:  {pane_id}");
    if let Some(a) = &args.alias {
        eprintln!("alias: {a}");
    }
    Ok(())
}

async fn open(mgr: Arc<SessionManager>, id: &str) -> anyhow::Result<()> {
    let lid = LinkId::parse(id)?;
    mgr.attach_link(lid).await?;
    println!("opened: {lid}");
    Ok(())
}

async fn close(mgr: Arc<SessionManager>, id: &str) -> anyhow::Result<()> {
    let lid = LinkId::parse(id)?;
    mgr.close_link(lid).await?;
    println!("closed: {lid}");
    Ok(())
}

async fn remove(mgr: Arc<SessionManager>, id: &str) -> anyhow::Result<()> {
    let lid = LinkId::parse(id)?;
    mgr.unregister_link(lid).await?;
    println!("removed: {lid}");
    eprintln!("(tmux server / ssh channel / child process still running; use 'link open' to re-attach)");
    Ok(())
}

async fn show(mgr: Arc<SessionManager>, id: &str) -> anyhow::Result<()> {
    let lid = LinkId::parse(id)?;
    let link = mgr.links.get(lid).await.ok_or(Error::LinkNotFound(lid))?;
    println!("{}", serde_json::to_string_pretty(&link.info)?);
    Ok(())
}

async fn list(mgr: Arc<SessionManager>) -> anyhow::Result<()> {
    for link in mgr.links.list().await {
        let info = &link.info;
        let alias = info.alias.clone().unwrap_or_default();
        let alias_field = if alias.is_empty() { String::new() } else { format!("\t{alias}") };
        println!("{}\t{}\t{}{}", info.id, info.transport_str(), info.name, alias_field);
    }
    Ok(())
}

async fn config_cmd(cmd: LinkConfigCmd) -> anyhow::Result<()> {
    match cmd {
        LinkConfigCmd::Add { name, url } => {
            let spec = url::parse(&url)?.0;
            xscore::config::ensure_dirs()?;
            xscore::config::save_connect_spec(
                &xscore::config::link_config_path(&name), &spec)?;
            println!("saved template: {name}");
        }
        LinkConfigCmd::Remove { name } => {
            let p = xscore::config::link_config_path(&name);
            if !p.exists() { anyhow::bail!("no such template: {name}"); }
            std::fs::remove_file(p)?;
            println!("removed template: {name}");
        }
        LinkConfigCmd::Show { name } => {
            let spec = xscore::config::load_connect_spec(&xscore::config::link_config_path(&name))?;
            println!("{}", serde_json::to_string_pretty(&spec)?);
        }
        LinkConfigCmd::List => {
            for p in xscore::config::list_link_configs()? {
                if let Some(s) = p.file_stem().and_then(|x| x.to_str()) {
                    println!("{s}");
                }
            }
        }
    }
    Ok(())
}

/// `link add -c <name>` means: take the saved template (with its transport + sink)
/// and override the transport with the CLI URL. The CLI URL still controls the
/// LinkAddress (used for `--link=existing` later). This lets users say:
///
///   xscore link config add web ssh://root@old-host:22
///   xscore link add ssh://root@new-host:22 -c web   # uses web's sink + new transport
fn merge_specs(template: crate::model::ConnectSpec, cli: crate::model::ConnectSpec) -> crate::model::ConnectSpec {
    // The CLI URL supplies the address. The CLI's transport detail (port, identity, ...)
    // wins because that's what the user just typed. The template supplies sink + name.
    crate::model::ConnectSpec {
        name: if cli.name.is_empty() { template.name } else { cli.name },
        transport: cli.transport,
        sink: template.sink,
    }
}
```

Note: `xscore::config::link_config_path`, `xscore::config::list_link_configs`, `xscore::config::load_connect_spec`, `xscore::config::save_connect_spec`, `xscore::config::ensure_dirs` are now used here. The paths land under `~/.config/xscore/links/` (see Step 5).

---

## Step 3: cli/pane.rs

```rust
// cli/pane.rs
use crate::core::{Error, LinkId, PaneId, SessionManager};
use clap::Subcommand;
use std::sync::Arc;

#[derive(Subcommand)]
pub enum PaneCmd {
    /// Open a new pane inside a link (sink.new_pane())
    Add {
        #[arg(long)] link: String,
        /// Optional human-friendly name (not persisted)
        #[arg(long)]
        name: Option<String>,
    },

    /// Re-attach to an existing tmux window (pane was already created by some link add or pane add)
    Open {
        #[arg(long)] id: String,
        /// Required if the link isn't in this registry (e.g. fresh process, restart)
        #[arg(long)]
        link: Option<String>,
    },

    /// Kill tmux pane + remove from registry
    Close { #[arg(long)] id: String },

    /// Remove from registry only (tmux window stays alive)
    Remove { #[arg(long)] id: String },

    /// Print PaneInfo JSON
    Show { #[arg(long)] id: String },

    /// List panes (optionally filtered by --link)
    List { #[arg(long)] link: Option<String> },
}

pub async fn run(mgr: Arc<SessionManager>, cmd: PaneCmd) -> anyhow::Result<()> {
    match cmd {
        PaneCmd::Add { link, name } => {
            let lid = LinkId::parse(&link)?;
            let pid = mgr.add_pane(lid, name).await?;
            println!("pane:  {pid}");
        }
        PaneCmd::Open { id, link } => {
            let pid = PaneId::parse(&id)?;
            // If link is supplied and not in registry, attach it first.
            if let Some(lid_str) = link {
                let lid = LinkId::parse(&lid_str)?;
                if mgr.links.get(lid).await.is_none() {
                    mgr.attach_link(lid).await?;
                }
            }
            mgr.attach_pane(pid).await?;
            println!("opened pane: {pid}");
        }
        PaneCmd::Close { id } => {
            let pid = PaneId::parse(&id)?;
            mgr.close_pane(pid).await?;
            println!("closed pane: {pid}");
        }
        PaneCmd::Remove { id } => {
            let pid = PaneId::parse(&id)?;
            mgr.unregister_pane(pid).await?;
            println!("removed pane: {pid}");
            eprintln!("(tmux window still alive; use 'pane open' to re-attach)");
        }
        PaneCmd::Show { id } => {
            let pid = PaneId::parse(&id)?;
            let info = mgr.panes.get(pid).await.ok_or(Error::PaneNotFound(pid))?;
            println!("{}", serde_json::to_string_pretty(&info)?);
        }
        PaneCmd::List { link } => {
            for p in mgr.panes.list_all().await {
                if let Some(lid) = &link {
                    if p.link_id.to_string() != *lid { continue; }
                }
                println!("{}\t{}\t{}", p.id, p.link_id, p.sink_handle);
            }
        }
    }
    Ok(())
}
```

---

## Step 4: main.rs

```rust
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();
    let cli = xscore::cli::Cli::parse();
    let mgr = std::sync::Arc::new(xscore::core::SessionManager::new());
    match cli.command {
        xscore::cli::Commands::Link(c) => xscore::cli::link::run(mgr, c).await?,
        xscore::cli::Commands::Pane(c) => xscore::cli::pane::run(mgr, c).await?,
        xscore::cli::Commands::Data(_c) => println!("data (todo)"),
        xscore::cli::Commands::Config(_c) => println!("config (todo)"),
    }
    Ok(())
}
```

---

## Step 5: New methods on `SessionManager` (Task 6 extension)

These are added to the existing `SessionManager` (Task 6 already has `close_link` / `close_pane`; rename `connect` → `add_link` and add 4 new methods):

```rust
// In core/manager.rs

impl SessionManager {
    /// Was: `connect`. Now: `add_link`. Returns `(link_id, pane_id)`.
    pub async fn add_link(
        &self,
        spec: ConnectSpec,
        address: LinkAddress,
        alias: Option<String>,
    ) -> Result<(LinkId, PaneId)> {
        // Same body as old `connect` but:
        //   - LinkInfo.alias = alias
        //   - LinkRegistry also inserts into by_alias if Some
        //   - Returns (link_id, pane_id)
        // See task-06 for the structural skeleton.
        let mut transport = transport_factory::transport_for(spec.transport.clone()).await?;
        let transport_kind = transport.kind();
        let mut sink = sink_for(&spec.sink, transport_kind, transport.as_mut()).await?;
        let mut transport = transport;
        sink.attach(transport.as_mut()).await?;

        let link_id = LinkId::new();
        let link_info = LinkInfo {
            id: link_id,
            name: if spec.name.is_empty() { link_id.to_string() } else { spec.name.clone() },
            alias: alias.clone(),
            created_at: chrono::Utc::now(),
            address: address.clone(),
            transport: spec.transport.clone(),
            sink: spec.sink.clone(),
        };
        let link = Link {
            id: link_id,
            info: link_info,
            transport,
            sink,
            panes: tokio::sync::Mutex::new(Vec::new()),
        };
        let arc_link = self.links.insert(link).await;

        // open first pane
        let sink_handle = arc_link.sink.new_pane().await?;
        let pane_index = arc_link.panes.lock().await.len() as u32;
        let pane_id = PaneId::new();
        let pane_info = PaneInfo {
            id: pane_id,
            link_id: arc_link.id,
            name: format!("pane-{pane_index}"),
            created_at: chrono::Utc::now(),
            sink_handle: sink_handle.clone(),
            pane_index,
        };
        arc_link.panes.lock().await.push(pane_id);
        self.panes.insert(pane_info).await;
        let backend: Arc<dyn PaneBackend> = Arc::new(CompositeBackend::new(pane_id, arc_link.clone(), sink_handle));
        self.backends.insert(pane_id, backend).await;

        Ok((arc_link.id, pane_id))
    }

    /// Re-attach to an existing link by id.
    /// For this slice: the link's underlying resources must already exist
    /// (tmux server / ssh channel / child / serial port); we just rebuild
    /// the in-memory transport/sink handles and re-register panes from
    /// sink.list_panes().
    pub async fn attach_link(&self, id: LinkId) -> Result<()> {
        // 1. Find persisted state under state/links/<id>.json if present.
        // 2. Build transport + sink from spec.
        // 3. Register back in LinkRegistry (with alias).
        // 4. For each pane from sink.list_panes(): re-register in
        //    PaneRegistry + BackendRegistry with a new PaneId.
        // For this slice the persistent-state path is stubbed:
        // we read state only if the file exists; if not, we error with
        // a hint to use 'link list' (which only shows in-memory).
        todo!("stub for slice; real impl lands in attach-persistence PR")
    }

    /// Unregister a link from in-memory state, leaving external resources alive.
    pub async fn unregister_link(&self, id: LinkId) -> Result<()> {
        let arc_link = self.links.get(id).await.ok_or(Error::LinkNotFound(id))?;
        // remove all panes from in-memory registries only
        let panes = arc_link.panes.lock().await.clone();
        for pid in panes {
            self.backends.remove(pid).await;
            self.panes.remove(pid).await;
        }
        // DO NOT call sink.close() or transport.close()
        self.links.remove(id).await;
        Ok(())
    }

    /// Add a new pane to an existing link.
    pub async fn add_pane(&self, link_id: LinkId, name: Option<String>) -> Result<PaneId> {
        let arc_link = self.links.get(link_id).await.ok_or(Error::LinkNotFound(link_id))?;
        let sink_handle = arc_link.sink.new_pane().await?;
        let pane_index = arc_link.panes.lock().await.len() as u32;
        let pane_id = PaneId::new();
        let pane_name = name.unwrap_or_else(|| format!("pane-{pane_index}"));
        let pane_info = PaneInfo {
            id: pane_id,
            link_id,
            name: pane_name,
            created_at: chrono::Utc::now(),
            sink_handle: sink_handle.clone(),
            pane_index,
        };
        arc_link.panes.lock().await.push(pane_id);
        self.panes.insert(pane_info).await;
        let backend: Arc<dyn PaneBackend> = Arc::new(CompositeBackend::new(pane_id, arc_link.clone(), sink_handle));
        self.backends.insert(pane_id, backend).await;
        Ok(pane_id)
    }

    /// Re-attach to an existing tmux pane by pane_id.
    /// For this slice: link must already be in the registry (use `pane open --link`).
    pub async fn attach_pane(&self, id: PaneId) -> Result<()> {
        // 1. Look up pane_info; get link_id + sink_handle
        // 2. Ensure link is in registry (caller passes link_id via `pane open --link`)
        // 3. Re-register backend in BackendRegistry
        // For this slice: stub; full implementation lands in attach persistence PR.
        todo!("stub for slice; real impl lands in attach-persistence PR")
    }

    /// Unregister a pane from in-memory state, leaving the tmux window alive.
    pub async fn unregister_pane(&self, id: PaneId) -> Result<()> {
        let backend = self.backends.remove(id).await.ok_or(Error::PaneNotFound(id))?;
        let info = self.panes.remove(id).await.ok_or(Error::PaneNotFound(id))?;
        // DO NOT call backend.close()
        drop(backend);
        if let Some(link) = self.links.get(info.link_id).await {
            link.panes.lock().await.retain(|p| *p != id);
        }
        Ok(())
    }

    /// Existing close_link / close_pane — same as Task 6.
    pub async fn close_link(&self, id: LinkId) -> Result<()> { /* task-06 */ }
    pub async fn close_pane(&self, id: PaneId) -> Result<()> { /* task-06 */ }
}
```

---

## Step 6: Config-path updates (Task 9 follow-up)

The old `sessions/link-<id>.json` path is gone. New layout:

| Path | Purpose | Lifetime |
| --- | --- | --- |
| `~/.config/xscore/links/<alias>.json` | ConnectSpec template (`link config add`) | persistent |
| `~/.config/xscore/state/links/<id>.json` | running-state per link (alias, panes) | optional this slice |
| `~/.config/xscore/config.json` | AppConfig | persistent |

Add to `config/paths.rs`:

```rust
pub fn link_config_path(alias: &str) -> std::path::PathBuf { links_dir().join(format!("{alias}.json")) }
pub fn links_dir() -> std::path::PathBuf { config_dir().join("links") }
pub fn state_dir() -> std::path::PathBuf { config_dir().join("state") }
pub fn state_link_path(id: &str) -> std::path::PathBuf { state_dir().join("links").join(format!("{id}.json")) }

pub fn ensure_dirs() -> std::io::Result<()> {
    std::fs::create_dir_all(config_dir())?;
    std::fs::create_dir_all(links_dir())?;
    Ok(())
}
```

Add to `config/loader.rs`:

```rust
pub fn list_link_configs() -> Result<Vec<std::path::PathBuf>> {
    let dir = super::paths::links_dir();
    if !dir.exists() { return Ok(vec![]); }
    let mut out = vec![];
    for entry in std::fs::read_dir(dir)? {
        let e = entry?;
        let p = e.path();
        if p.extension().and_then(|x| x.to_str()) == Some("json") {
            out.push(p);
        }
    }
    out.sort();
    Ok(out)
}
```

Update `config/mod.rs` to add the new exports.

---

## Step 7: LinkInfo gains `alias`

```rust
// model/session.rs — LinkInfo

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct LinkInfo {
    pub id: LinkId,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub alias: Option<String>,    // NEW: runtime alias (in-memory only)
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub address: LinkAddress,
    pub transport: TransportConfig,
    pub sink: SinkConfig,
}
```

---

## Step 8: LinkRegistry gains `by_alias`

```rust
// core/link_registry.rs — internal state

struct LinkRegistryInner {
    by_id: HashMap<LinkId, ArcLink>,
    by_address: HashMap<LinkAddress, LinkId>,
    by_alias: HashMap<String, LinkId>,   // NEW
}

impl LinkRegistry {
    // update insert/remove to also write by_alias when info.alias.is_some()
    pub async fn find_by_alias(&self, alias: &str) -> Option<ArcLink> {
        let g = self.inner.read().await;
        let id = g.by_alias.get(alias).copied()?;
        g.by_id.get(&id).cloned()
    }
}
```

`insert()` writes `by_alias` when `info.alias.is_some()`. `remove()` drops from both `by_id` and `by_alias`.

---

## Step 9: Smoke

```bash
# Persistent template
cargo run -p xscore -- link config add web ssh://root@host:22?identity=k1
cargo run -p xscore -- link config list
cargo run -p xscore -- link config show web
cargo run -p xscore -- link config remove web

# Lifecycle
cargo run -p xscore -- link add 'shell:///bin/cat' -a cat1            # create
cargo run -p xscore -- link list                                       # see cat1
cargo run -p xscore -- link show --id <link-id>

cargo run -p xscore -- pane add --link <link-id> --name extra          # open pane 2
cargo run -p xscore -- pane list --link <link-id>
cargo run -p xscore -- pane close --id <pane-id>                       # kill tmux pane
cargo run -p xscore -- pane remove --id <pane-id>                      # only unregister

cargo run -p xscore -- link close --id <link-id>                       # kill transport + sink + all panes
cargo run -p xscore -- link remove --id <link-id>                      # only unregister (preserves tmux server)
cargo run -p xscore -- link open --id <link-id>                        # re-attach (uses sink.list-panes())
```

---

## Step 10: Commit

```bash
git add crates/xscore/src/cli/ crates/xscore/src/core/ crates/xscore/src/config/ crates/xscore/src/model/ crates/xscore/src/main.rs
git commit -m "feat(cli): two-tier command tree (link add/open/close/remove/show/list, pane add/open/close/remove/show/list, link config add/remove/show/list)"
```