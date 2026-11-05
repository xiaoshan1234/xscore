//! 常驻 daemon：持有 live 连接与 pane 状态，让一次性 CLI 进程退出后数据仍在。

pub mod client;
pub mod ipc;
pub mod protocol;
pub mod server;

use anyhow::Result;

use crate::cli::DaemonCmd;

/// 根据 `daemon` 子命令执行启动、状态查询或停止。
///
/// 参数/输出：`cmd` 为输入；成功时无返回值。
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
