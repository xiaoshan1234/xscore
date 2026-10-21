// cli/mod.rs
//
// Top-level CLI dispatch. The verb groups are:
//   - session (v0.6.0: the workspace that owns windows/panes/links — ADR 0010)
//   - link    (4-verb + config subgroup, ADR 0005)
//   - window  (4-verb, ADR 0006)
//   - pane    (4-verb with --window, ADR 0006)
//   - data    (per-pane read / write / resize)
//   - daemon  (v0.5.0: the resident process that owns direct-link
//             transports and pane buffers — ADR 0009)
//   - state   (v0.6.0: schema migration — `state migrate`, ADR 0010 D8)
//   - config  (stub — `xscore config {show, path, set, unset}` deferred to
//             a later task; for this slice the `xscore config` shell
//             shows a TODO and exits non-zero so users get a clear
//             message)

pub mod daemon;
pub mod data;
pub mod link;
pub mod pane;
pub mod session;
pub mod state;
pub mod window;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "xscore", version, about = "Manage shell / ssh / serial / tmux sessions as Session → Window → Pane → Link (ADR 0010)")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Subcommand)]
pub enum Commands {
    /// Inspect / create / kill sessions (the workspace that owns windows, panes and links)
    #[command(subcommand)]
    Session(session::SessionCmd),
    /// Inspect / create / remove links (one connection each: a pipe, or a session's control client)
    #[command(subcommand)]
    Link(link::LinkCmd),
    /// Inspect / create / remove windows (layout containers inside a session; a tmux window is mirrored)
    #[command(subcommand)]
    Window(window::WindowCmd),
    /// Inspect / create / remove panes (a buffer with a viewport; its bytes come from its link)
    #[command(subcommand)]
    Pane(pane::PaneCmd),
    /// Read / write / resize data on a pane
    #[command(subcommand)]
    Data(data::DataCmd),
    /// Inspect / stop the resident daemon that owns every live connection
    #[command(subcommand)]
    Daemon(daemon::DaemonCmd),
    /// Inspect / migrate the on-disk state schema
    #[command(subcommand)]
    State(state::StateCmd),
    /// Show / edit AppConfig values (deferred — not yet implemented)
    Config,
}
