//! `xs` client: forward raw argv to the resident daemon and print its output.

mod daemon;
mod secrets;

use std::io::Write as _;

use anyhow::Result;

#[tokio::main]
async fn main() -> Result<()> {
    let mut argv: Vec<String> = std::env::args().skip(1).collect();
    if argv.is_empty() {
        argv.push("--help".to_string());
    }

    if argv.first().map(String::as_str) == Some("daemon") {
        return daemon::run_bootstrap(&argv).await;
    }

    secrets::resolve_secrets(&mut argv)?;
    match daemon::run_command(argv).await {
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
