//! 后端统一错误面。

use thiserror::Error;

/// 后端统一错误类型。
#[derive(Debug, Error)]
pub enum Error {
    /// 本地 I/O、进程或文件系统操作失败。
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    /// 网络、SSH、PTY 或外部进程通信失败。
    #[error("transport error: {0}")]
    Transport(String),

    /// 调用方提供的配置或输入不合法。
    #[error("invalid input: {0}")]
    Invalid(String),
}

/// 后端统一结果类型。
pub type Result<T> = std::result::Result<T, Error>;
