// app/cli/daemon.rs (v0.5.0 — resident daemon, ADR 0009/0010)
//
// `xscore daemon serve|status|stop`.
//
// `serve` is normally started for you: the first command that needs a
// transport or a tmux control client (`link add`, `session new --tmux`,
// `pane attach`) auto-starts it and waits for the socket. `status`/`stop`
// are the manual handle on it.

use clap::Subcommand;

use crate::service::daemon::{client, protocol::Request};

#[derive(Subcommand)]
pub enum DaemonCmd {
    /// Run the daemon in the foreground (auto-start uses this).
    ///
    /// Holds every live transport, every tmux session's control client and
    /// every pane's byte buffer, so they survive the CLI process that
    /// created them. Stays up until `xscore daemon stop`, Ctrl-C or SIGTERM;
    /// the auto-started copy additionally leaves once it has nothing left to
    /// supervise, which is what `--idle-exit` selects.
    Serve {
        /// Exit after 5 s with no links and no tmux clients (the mode
        /// auto-start uses; a bare `serve` is expected to stay up).
        #[arg(long)]
        idle_exit: bool,
    },

    /// Print the daemon's status as JSON (pid, links, panes + buffered
    /// byte counts, and one entry per tmux session). Exits non-zero if no
    /// daemon is running.
    Status,

    /// Ask the daemon to exit (it closes its transports and tmux clients,
    /// and removes its socket). Direct links then report `connected: false`,
    /// and tmux pane buffers are gone — they are memory.
    Stop,
}

pub async fn run(cmd: DaemonCmd) -> anyhow::Result<()> {
    match cmd {
        DaemonCmd::Serve { idle_exit } => {
            crate::service::daemon::server::run(idle_exit).await?;
        }
        DaemonCmd::Status => {
            let resp = client::send(&Request::Status).await?;
            if !resp.ok {
                anyhow::bail!(
                    "daemon refused status: {}",
                    resp.error.unwrap_or_else(|| "unknown error".into())
                );
            }
            // One line per tmux session on stderr, so stdout stays the JSON a
            // script (or a test) can parse while a human can still see what the
            // daemon is doing.
            if let Some(status) = &resp.status {
                for t in &status.tmux {
                    eprintln!(
                        "tmux {}: {} alive={} panes={} buffered={}",
                        t.session, t.name, t.alive, t.panes, t.buffered
                    );
                }
            }
            println!("{}", serde_json::to_string_pretty(&resp)?);
        }
        DaemonCmd::Stop => {
            // A missing daemon is not a failure for `stop`: the requested
            // state ("nothing running") already holds. Tests rely on that.
            match client::send(&Request::Stop).await {
                Ok(resp) if resp.ok => println!("daemon stopped"),
                Ok(resp) => anyhow::bail!(
                    "daemon refused stop: {}",
                    resp.error.unwrap_or_else(|| "unknown error".into())
                ),
                Err(_) => println!("daemon not running"),
            }
        }
    }
    Ok(())
}
