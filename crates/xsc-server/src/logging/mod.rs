//! 进程日志初始化与 pane 输出 logger。

mod logger;
mod setup;

pub use logger::{default_path, LoggerFormat, LoggerInfo, LoggerRegistry};
pub use setup::init;
