//! 前端入口：askpass 角色、日志初始化、模式路由（一次性命令 / daemon）。

mod app;
mod cli;
mod daemon;
mod layout;
mod logging;
mod output;
mod tmux_events;

use std::io::Write as _;

use anyhow::Result;

use cli::Cmd;

#[tokio::main]
async fn main() -> Result<()> {
    // 作为 SSH_ASKPASS 辅助程序被 ssh 调用时，输出密码并退出。
    if std::env::var("XSCORE_ASKPASS").is_ok() {
        if let Ok(password) = std::env::var("XSCORE_SSH_PASSWORD") {
            println!("{password}");
        }
        return Ok(());
    }
    if let Err(error) = logging::init() {
        eprintln!("warning: logging disabled: {error:#}");
    }

    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() {
        cli::print_help()?;
        return Ok(());
    }

    let cmd = match cli::parse_args(args) {
        Ok(cmd) => cmd,
        Err(error) => error.exit(),
    };
    if let Cmd::Daemon(daemon_cmd) = cmd {
        return daemon::run_bootstrap(daemon_cmd).await;
    }

    match daemon::client::run_command(cmd).await {
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
