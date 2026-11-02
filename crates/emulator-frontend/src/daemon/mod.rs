//! 常驻 daemon：持有 live 连接与 pane 状态，让一次性 CLI 进程退出后数据仍在。

pub mod client;
pub mod ipc;
pub mod protocol;
pub mod server;

use anyhow::Result;

use crate::cli::DaemonCmd;

pub async fn run_bootstrap(cmd: DaemonCmd) -> Result<()> {
    match cmd {
        DaemonCmd::Serve => server::run().await,
        DaemonCmd::Status => {
            let output = client::daemon_status().await?;
            print!("{output}");
            Ok(())
        }
        DaemonCmd::Stop => match client::daemon_stop().await? {
            true => {
                println!("daemon stopped");
                Ok(())
            }
            false => {
                println!("daemon not running");
                Ok(())
            }
        },
    }
}
