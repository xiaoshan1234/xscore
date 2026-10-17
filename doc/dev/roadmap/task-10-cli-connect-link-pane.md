# Task 10: CLI — `link`, `pane` subcommands

**Objective:** Wire the new two-tier CLI: `link {add,del,open,close,show,list,config ...}` + `pane {add,del,show,list}`. ADR 0005 retired `--link=existing`; `link` verbs split into `add` (always new), `open` (always attach-existing, tmux only), `close` (tmux-detach only), `del` (full teardown). Pane verbs trimmed to a 4-verb tree mirroring the link shape: `add` (new), `del` (full teardown), `show`, `list`.

**Background:** this is the revised CLI surface from the user-feedback round (after ADR 0005's reversal of `--link=existing`). The old `xscore connect <url>` is now `xscore link add <url>` (always-new). Re-attach-to-existing moved to `xscore link open` (tmux only). The `link config {add,remove,show,list}` group keeps managing persistent ConnectSpec templates under `~/.config/xscore/links/` unchanged.

**Files:**
- Create: `crates/xscore/src/cli/{mod,link,pane}.rs`
- Delete: `crates/xscore/src/cli/connect.rs` (replaced by `link::add`)
- Delete: `crates/xscore/src/cli/data.rs` (handled in Task 11)
- Modify: `crates/xscore/src/main.rs`
- Modify: `crates/xscore/src/core/manager.rs` (per ADR 0005: drop `link_existing` from `add_link`, rename `attach_link` → `open_link`, rename old `close_link` → `del_link`, redefine `close_link` as in-memory detach)
- Modify: `crates/xscore/src/config/{paths,loader}.rs` (new `links/` + `state/` paths)

---

## Command reference

```
xscore link add <url?> [-c CONFIG]              # always create new; url OR -c CONFIG required
xscore link del --id <link-id>                  # full teardown: kill transport + sink + every pane
xscore link open <url?> [-c CONFIG]             # attach existing; tmux only — others error OpenOnlyForTmux
xscore link close --id <link-id>                # tmux-detach only (unregister in-memory); others error CloseOnlyForTmux
xscore link show --id <link-id>
xscore link list

xscore link config add <name> <url>              # save ConnectSpec template (no start)
xscore link config remove <name>                 # delete template
xscore link config show <name>                   # read template
xscore link config list                          # list templates

xscore pane add --link <link-id> [--name=NAME]          # new_pane() inside the link
xscore pane del --id <pane-id>                      # kill-pane + unregister (full teardown)
xscore pane show --id <pane-id>
xscore pane list [--link <link-id>]

xscore data read --pane <pane-id> [--bytes N]
xscore data write --pane <pane-id> [--from-file FILE]
xscore data resize --pane <pane-id> --cols C --rows R

xscore config show
xscore config path
xscore config set <key> <value>
xscore config unset <key>
```

### add vs open — create vs re-attach (semantics table)

| Verb | Creates a new external resource? | Touches in-memory registry? | Allowed transports | Forgetting this leaks: |
| --- | --- | --- | --- | --- |
| `link add`    | yes (transport + sink + 1 pane)   | yes | all                          | processes / SSH channels / tmux sessions |
| `link del`    | no (kills them)                   | yes | all                          | nothing — already gone |
| `link open`   | no (re-attaches existing tmux)    | yes | tmux only — others err       | nothing — already running |
| `link close`  | no (detaches — leaves tmux alive) | yes | tmux only — others err       | nothing — per-link session stays in shared server |
| `pane add`    | yes (one tmux window)             | yes | tmux only                    | tmux window |
| `pane del`    | no (kills the tmux window)        | yes | tmux only                    | nothing — window is gone |

**Naming intent:** Linux file semantics adapted — verbs split into `add` (create new) and `del` (universal teardown). Both `link` and `pane` follow the same 4-verb shape: `add` / `del` / `show` / `list`. The tmux-only "open" and "close" attach/detach verbs are exclusive to `link` because that's where the cross-invocation identity lives.

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
    let (link_id, pane_id) = mgr.add_link(spec, address).await?;
    eprintln!("link:  {link_id}");
    println!("pane:  {pane_id}");
    Ok(())
}

async fn open(mgr: Arc<SessionManager>, args: LinkOpenArgs) -> anyhow::Result<()> {
    // Same input resolution as `add`. The manager rejects non-tmux with `OpenOnlyForTmux`.
    let (spec, address) = resolve_link_input(args.url.as_deref(), args.config.as_deref())?;
    let (link_id, pane_id) = mgr.open_link(spec, address).await?;
    println!("link:  {link_id}");
    println!("pane:  {pane_id}");
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
    for link in mgr.links.list().await {
        let info = &link.info;
        println!("{}\t{}", info.id, info.name);
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

/// Resolve `(url?, -c CONFIG?)` to `(ConnectSpec, LinkAddress)`. Used by
/// both `link add` and `link open`. At least one input is required.
/// Precedence (highest first): URL wins on transport detail; template
/// supplies sink + name when present.
///
///   # URL only
///   xscore link add ssh://root@new-host:22
///
///   # Template only (URL derived from the saved ConnectSpec)
///   xscore link open web            # resolves to whatever URL `web` was saved with
///
///   # URL + template (CLI URL overrides transport detail; template supplies sink + name)
///   xscore link config add web ssh://root@old-host:22
///   xscore link add ssh://root@new-host:22 -c web
fn resolve_link_input(
    url: Option<&str>,
    config_name: Option<&str>,
) -> anyhow::Result<(crate::model::ConnectSpec, crate::model::LinkAddress)> {
    match (url, config_name) {
        (None, None) => anyhow::bail!("provide either <url> or -c CONFIG"),
        (Some(u), None) => url::parse(u),
        (None, Some(c)) => {
            let template_path = xscore::config::link_config_path(c);
            let spec = xscore::config::load_connect_spec(&template_path)?;
            // The saved template already encodes the transport tuple and sink
            // — its `address` is what was registered when the template was made.
            let address = spec.address.clone();
            Ok((spec, address))
        }
        (Some(u), Some(c)) => {
            let template_path = xscore::config::link_config_path(c);
            let base_spec = xscore::config::load_connect_spec(&template_path)?;
            let (cli_spec, cli_addr) = url::parse(u)?;
            Ok((merge_specs(base_spec, cli_spec), cli_addr))
        }
    }
}

/// `link add -c <name> <url>` means: take the saved template (with its
/// transport + sink) and override the transport with the CLI URL. The
/// CLI URL still controls the LinkAddress. This lets users say:
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
use crate::core::{Error, PaneId, SessionManager};
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

    /// Kill the tmux window + remove from registry (full teardown).
    /// Cross-invocation re-attach goes through `xscore link open`, not through panes.
    Del { #[arg(long)] id: String },

    /// Print PaneInfo JSON
    Show { #[arg(long)] id: String },

    /// List panes (optionally filtered by --link)
    List { #[arg(long)] link: Option<String> },
}

pub async fn run(mgr: Arc<SessionManager>, cmd: PaneCmd) -> anyhow::Result<()> {
    match cmd {
        PaneCmd::Add { link, name } => {
            let lid = crate::core::LinkId::parse(&link)?;
            let pid = mgr.add_pane(lid, name).await?;
            println!("pane:  {pid}");
        }
        PaneCmd::Del { id } => {
            let pid = PaneId::parse(&id)?;
            mgr.del_pane(pid).await?;
            println!("deleted pane: {pid}");
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

## Step 5: Methods on `SessionManager` (per ADR 0005)

Task 6 establishes `add_link` / `open_link` / `close_link` / `del_link` as the four link lifecycle verbs and `add_pane` / `del_pane` as the two-pane lifecycle verbs. Step 5 of this task mirrors them so the CLI can dispatch. Pane `attach_pane` (re-attach stub) and `unregister_pane` are gone — the `pane` subcommand was trimmed to a 4-verb tree (`add` / `del` / `show` / `list`); cross-invocation re-attach happens at the link level via `xscore link open`.

```rust
// In core/manager.rs

impl SessionManager {
    /// Was: `connect`. Now: `add_link`. Returns `(link_id, pane_id)`.
    /// Always creates a brand-new external resource (ADR 0005 retired `--link=existing`).
    pub async fn add_link(
        &self,
        spec: ConnectSpec,
        address: LinkAddress,
    ) -> Result<(LinkId, PaneId)> {
        // Body as in Task 6 §Step 7 — same code path.
        let mut transport = transport_factory::transport_for(spec.transport.clone()).await?;
        let transport_kind = transport.kind();
        let mut sink = sink_for(&spec.sink, transport_kind, transport.as_mut()).await?;
        let mut transport = transport;
        sink.attach(transport.as_mut()).await?;

        let link_id = LinkId::new();
        let link_info = LinkInfo {
            id: link_id,
            name: if spec.name.is_empty() { link_id.to_string() } else { spec.name.clone() },
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

    /// Attach to an existing tmux session (ADR 0005). Non-tmux returns
    /// `Error::OpenOnlyForTmux(<kind>)`. Local-tmux case wires up; remote-
    /// tmux (ssh) lands in the SSH-integration phase.
    pub async fn open_link(
        &self,
        spec: ConnectSpec,
        address: LinkAddress,
    ) -> Result<(LinkId, PaneId)> {
        // Body as in Task 6 — same code path.
        match address.kind() {
            "shell" | "serial" | "ssh" => Err(Error::OpenOnlyForTmux(address.kind())),
            "tmux" => {
                // Local tmux case: lookup the session in the shared tmux server (identified by spec.sink)
                // identified by spec.sink; if found, register the link without
                // creating a session; if not found, return LinkNotFound.
                todo!("stub for slice; real impl lands in tmux-attach persistence PR")
            }
            _ => unreachable!(),
        }
    }

    /// Detach a tmux link from the in-memory registry (ADR 0005).
    /// Tmux session keeps running. Non-tmux returns `Error::CloseOnlyForTmux`.
    pub async fn close_link(&self, id: LinkId) -> Result<()> {
        let arc_link = self.links.get(id).await.ok_or(Error::LinkNotFound(id))?;
        let kind = arc_link.info.address.kind();
        if kind != "tmux" {
            return Err(Error::CloseOnlyForTmux(kind));
        }
        // In-memory-only detach. Do NOT call sink.close() / transport.close().
        let panes = arc_link.panes.lock().await.clone();
        for pid in panes {
            self.backends.remove(pid).await;
            self.panes.remove(pid).await;
        }
        self.links.remove(id).await;
        Ok(())
    }

    /// Full teardown: kill transport + sink + every pane, plus (for tmux)
    /// the per-link tmux session (the shared `-L xscore` server stays up). Was previously named `close_link`; renamed
    /// to `del_link` to match the CLI verb (ADR 0005).
    pub async fn del_link(&self, id: LinkId) -> Result<()> {
        let arc_link = self.links.get(id).await.ok_or(Error::LinkNotFound(id))?;
        let panes = arc_link.panes.lock().await.clone();
        for pid in panes {
            self.backends.remove(pid).await;
            self.panes.remove(pid).await;
        }
        arc_link.sink.close().await?;
        arc_link.transport.close().await?;
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

    /// Full teardown of a pane: kill the tmux window + remove the
    /// backend + unregister from in-memory. Was previously named
    /// `close_pane`; renamed to `del_pane` to match the CLI verb.
    /// Cross-invocation re-attach goes through `xscore link open`
    /// at the link level, not through panes.
    pub async fn del_pane(&self, id: PaneId) -> Result<()> {
        let backend = self.backends.remove(id).await.ok_or(Error::PaneNotFound(id))?;
        let info = self.panes.remove(id).await.ok_or(Error::PaneNotFound(id))?;
        backend.close().await?;
        if let Some(link) = self.links.get(info.link_id).await {
            link.panes.lock().await.retain(|p| *p != id);
        }
        Ok(())
    }
}
```

---

## Step 6: Config-path updates (Task 9 follow-up)

The old `sessions/link-<id>.json` path is gone. New layout:

| Path | Purpose | Lifetime |
| --- | --- | --- |
| `~/.config/xscore/links/<name>.json` | ConnectSpec template (`link config add`) | persistent |
| `~/.config/xscore/state/links/<id>.json` | running-state per link (panes) | optional this slice |
| `~/.config/xscore/config.json` | AppConfig | persistent |

Add to `config/paths.rs`:

```rust
pub fn link_config_path(name: &str) -> std::path::PathBuf { links_dir().join(format!("{name}.json")) }
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

## Step 7: Smoke

```bash
# Persistent template
cargo run -p xscore -- link config add web ssh://root@host:22?identity=k1
cargo run -p xscore -- link config list
cargo run -p xscore -- link config show web
cargo run -p xscore -- link config remove web

# Lifecycle (add / del — universal verbs)
cargo run -p xscore -- link add 'shell:///bin/cat'                       # create
cargo run -p xscore -- link list                                        # see new link
cargo run -p xscore -- link show --id <link-id>

cargo run -p xscore -- pane add --link <link-id> --name extra          # open pane 2
cargo run -p xscore -- pane list --link <link-id>
cargo run -p xscore -- pane del --id <pane-id>                         # kill tmux pane + unregister

# Tmux-only verbs (open = attach existing, close = detach)
cargo run -p xscore -- link open 'tmux://my-session'                   # attach existing
cargo run -p xscore -- link close --id <link-id>                       # tmux-detach only
cargo run -p xscore -- link del --id <link-id>                         # full teardown
```

---

## Step 8: Commit

```bash
git add crates/xscore/src/cli/ crates/xscore/src/core/ crates/xscore/src/config/ crates/xscore/src/model/ crates/xscore/src/main.rs
git commit -m "feat(cli): 4-verb link + 4-verb pane trees (add/del/show/list)"
```