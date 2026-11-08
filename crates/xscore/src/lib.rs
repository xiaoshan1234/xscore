//! 可复用的终端连接与传输库（L0–L1）。
//!
//! `xscore` 统一提供本地 PTY/ConPTY、SSH shell 和 tmux 控制连接。上层通过
//! [`Domain`] 创建终端，通过 [`TerminalHandle`] 收发原始字节；VT 解析、布局和
//! 图形渲染由调用方负责。
//!
//! 分层严格单向：`platform`（L0）← `domain`（L1）。
//!
//! # 快速开始
//!
//! ```no_run
//! use xscore::prelude::*;
//!
//! #[tokio::main(flavor = "current_thread")]
//! async fn main() -> Result<()> {
//!     let domain = LocalDomain::new();
//!     let terminal = domain
//!         .spawn_terminal(&TerminalConfig::local_command(
//!             "sh",
//!             ["-c", "printf 'hello from xscore\\n'"],
//!         ))
//!         .await?;
//!
//!     let mut output = terminal.subscribe();
//!     while let Ok(chunk) = output.recv().await {
//!         print!("{}", String::from_utf8_lossy(&chunk));
//!     }
//!     terminal.close().await?;
//!     Ok(())
//! }
//! ```
//!
//! 完整的依赖配置、SSH/tmux 示例和平台限制见 crate 包内的 `README.md`。

#![warn(missing_docs)]

pub mod domain;
pub mod error;
pub mod platform;

pub use error::{Error, Result};

pub use domain::{
    enumerate, make_terminal, Domain, LocalDomain, LocalTerminal, RemoteSshDomain, SshTerminal,
    TerminalConfig, TerminalHandle, TerminalId, TerminalKind, TmuxDomain, TmuxTerminal,
    TmuxTerminalInfo, TmuxWindowInfo,
};
pub use platform::ssh::SshOptions;
pub use platform::tmux::{TmuxControl, TmuxEvent, TmuxHostConfig, TmuxSocket, TmuxSpawnMode};
pub use platform::{LocalPty, Size};

/// 常用类型的统一导入入口。
///
/// 大多数调用方只需 `use xscore::prelude::*;`。需要底层协议类型时，可从根模块或
/// [`platform`] 模块显式导入。
pub mod prelude {
    pub use crate::{
        Domain, Error, LocalDomain, RemoteSshDomain, Result, Size, SshOptions, TerminalConfig,
        TerminalHandle, TerminalId, TerminalKind, TmuxDomain, TmuxEvent, TmuxHostConfig,
        TmuxSocket,
    };
}
