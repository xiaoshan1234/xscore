//! 前端入口：askpass 角色、日志初始化、模式路由（一次性命令 / daemon）。

mod cli;
mod daemon;
mod logging;
mod ui;

use std::io::Write as _;

use anyhow::Result;

use cli::Cmd;

#[tokio::main]
async fn main() -> Result<()> {
    // 作为 SSH_ASKPASS 辅助程序被 ssh 调用时，从受限临时文件读取密码。
    if let Some(path) = std::env::var_os("XSCORE_ASKPASS_FILE") {
        let password = std::fs::read(path)?;
        let stdout = std::io::stdout();
        let mut stdout = stdout.lock();
        stdout.write_all(&password)?;
        if !password.ends_with(b"\n") {
            stdout.write_all(b"\n")?;
        }
        stdout.flush()?;
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
