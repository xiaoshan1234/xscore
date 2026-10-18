// main.rs — the binary entry point.
//
// All command parsing is done via clap (in `cli::Cli`); this file is just
// the bootstrap that wires the parser output to the manager methods. The
// manager holds the in-memory registry; in a future task it'll be hydrated
// from `~/.config/xscore/state/` on startup and persisted on shutdown
// (deferred to a persistence slice — task-15+ territory).

use std::sync::Arc;
use clap::Parser;
use xscore::cli::{link, pane, window, Cli, Commands};
use xscore::core::SessionManager;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // tracing_subscriber is a thin layer; init() picks up RUST_LOG.
    // No-op if the env var is unset — quiet by default, verbose with RUST_LOG=info.
    let _ = tracing_subscriber::fmt::try_init();
    let cli = Cli::parse();

    // Hydrate the manager from disk (task-15). On first run, this returns
    // an empty manager. After `xscore link add`, state is saved and the
    // next invocation sees the link.
    let mgr = Arc::new(SessionManager::load_from_state().await?);
    match cli.command {
        Commands::Link(c) => link::run(mgr, c).await?,
        Commands::Window(c) => window::run(mgr, c).await?,
        Commands::Pane(c) => pane::run(mgr, c).await?,
        Commands::Data(c) => {
            // Per-pane data ops (read / write / resize) — task-11.
            xscore::cli::data::run(mgr, c).await?;
        }
        Commands::Config => {
            // AppConfig is a separate concern; deferred to a later task.
            // Without this stub, clap would reject `xscore config` as an
            // unknown subcommand.
            anyhow::bail!("`xscore config` is not yet implemented");
        }
    }
    Ok(())
}
