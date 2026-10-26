// main.rs — the binary entry point.
//
// All command parsing is done via clap (in `cli::Cli`); this file is just
// the bootstrap that wires the parser output to the manager methods.
//
// v0.5.0 (ADR 0009): the manager is built with the resident daemon
// enabled, so direct links (shell/serial/ssh) hand their transport and
// their pane buffers to `xscore daemon serve` instead of holding them
// here. Without that, every `data write` in a new process was impossible
// by construction: the transport died with the process that created it.

use clap::Parser;
use std::sync::Arc;
use xscore::app::cli::{buffer, link, pane, session, window, Cli, Commands};
use xscore::service::session_manager::SessionManager;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // tracing_subscriber is a thin layer; init() picks up RUST_LOG.
    // No-op if the env var is unset — quiet by default, verbose with RUST_LOG=info.
    let _ = tracing_subscriber::fmt::try_init();
    let cli = Cli::parse();

    // `daemon serve` must NOT hydrate state: it owns no metadata, and a
    // stale/incompatible state.json must not stop the daemon from coming
    // up (that would take every direct link down with it).
    //
    // `state migrate` must not hydrate either, for the mirror-image reason:
    // the file it exists to convert is the file hydration would reject.
    if matches!(&cli.command, Commands::Daemon(_) | Commands::State(_)) {
        match cli.command {
            Commands::Daemon(cmd) => return xscore::app::cli::daemon::run(cmd).await,
            Commands::State(cmd) => return xscore::app::cli::state::run(cmd),
            _ => unreachable!("checked by matches! above"),
        }
    }

    // Hydrate the manager from disk (task-15). On first run, this returns
    // an empty manager. After `xscore link add`, state is saved and the
    // next invocation sees the link.
    let mut mgr = SessionManager::load_from_state().await?;
    // v0.5.0: direct links outlive this process — the daemon owns them.
    mgr.enable_daemon(Arc::new(xscore::infra::daemon::DaemonClient::new()));
    let mgr = Arc::new(mgr);
    match cli.command {
        Commands::Session(c) => session::run(mgr, c).await?,
        Commands::Link(c) => link::run(mgr, c).await?,
        Commands::Buffer(c) => buffer::run(mgr, c).await?,
        Commands::Window(c) => window::run(mgr, c).await?,
        Commands::Pane(c) => pane::run(mgr, c).await?,
        Commands::Data(c) => {
            // Per-pane data ops (read / write / resize) — task-11.
            xscore::app::cli::data::run(mgr, c).await?;
        }
        Commands::Daemon(_) => unreachable!("handled before hydration"),
        Commands::State(_) => unreachable!("handled before hydration"),
        Commands::Config => {
            // AppConfig is a separate concern; deferred to a later task.
            // Without this stub, clap would reject `xscore config` as an
            // unknown subcommand.
            anyhow::bail!("`xscore config` is not yet implemented");
        }
    }
    Ok(())
}
