// cli/daemon.rs (v0.5.0 — resident daemon, ADR 0009)
//
// `xscore daemon serve|status|stop`.
//
// `serve` is normally started for you: the first command that needs a
// direct-link transport (`link add`, `pane attach`) auto-starts it and
// waits for the socket. `status`/`stop` are the manual handle on it.

use clap::Subcommand;

use crate::daemon::{client, protocol::Request};

#[derive(Subcommand)]
pub enum DaemonCmd {
    /// Run the daemon in the foreground (auto-start uses this).
    ///
    /// Holds every live direct-link transport and every pane's byte
    /// buffer, so they survive the CLI process that created them. Exits
    /// after `daemon stop`, or after 5 s idle with no links.
    Serve,

    /// Print the daemon's status as JSON (pid, links, panes + buffered
    /// byte counts). Exits non-zero if no daemon is running.
    Status,

    /// Ask the daemon to exit (it closes its transports and removes its
    /// socket). Direct links then report `connected: false`.
    Stop,
}

pub async fn run(cmd: DaemonCmd) -> anyhow::Result<()> {
    match cmd {
        DaemonCmd::Serve => {
            crate::daemon::server::run().await?;
        }
        DaemonCmd::Status => {
            let resp = client::send(&Request::Status).await?;
            if !resp.ok {
                anyhow::bail!(
                    "daemon refused status: {}",
                    resp.error.unwrap_or_else(|| "unknown error".into())
                );
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
