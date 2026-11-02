//! 交互式 REPL：解析输入后交给 daemon 执行，从而与一次性 CLI 共享状态。

use std::io::Write as _;

use anyhow::Result;

use crate::cli::{self, Cmd};
use crate::daemon::client;

pub async fn run() -> Result<()> {
    let stdin = std::io::stdin();
    loop {
        print!("xscore> ");
        std::io::stdout().flush()?;
        let mut line = String::new();
        if stdin.read_line(&mut line)? == 0 {
            break;
        }
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let cmd = match cli::parse_line(line) {
            Ok(cmd) => cmd,
            Err(error) => {
                error.print()?;
                continue;
            }
        };
        if matches!(&cmd, Cmd::Quit | Cmd::Exit) {
            break;
        }
        match client::run_command(cmd).await {
            Ok(output) => print!("{output}"),
            Err(error) => eprintln!("error: {error:#}"),
        }
    }
    Ok(())
}
