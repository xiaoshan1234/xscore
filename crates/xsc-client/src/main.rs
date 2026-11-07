//! `xs` client: parse one command and dispatch it to the resident daemon.

mod cli;
mod daemon;
mod wire;

use std::io::Write as _;

use anyhow::Result;

use cli::Cmd;

#[tokio::main]
async fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() {
        cli::print_help()?;
        return Ok(());
    }

    let cmd = match cli::parse_args(args) {
        Ok(cmd) => cmd,
        Err(error) => error.exit(),
    };
    if let Cmd::Daemon(daemon_cmd) = &cmd {
        return daemon::run_bootstrap(daemon_cmd).await;
    }

    match daemon::run_command(cmd).await {
        Ok(output) => {
            print!("{output}");
            std::io::stdout().flush()?;
            Ok(())
        }
        Err(error) => {
            eprintln!("error: {error:#}");
            std::process::exit(1);
        }
    }
}
