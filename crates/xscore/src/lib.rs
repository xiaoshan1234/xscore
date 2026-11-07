//! 终端连接与传输后端（L0–L1）。
//!
//! 分层严格单向：`platform`（L0）← `domain`（L1）。VT 状态模型归前端侧。
//! 权威架构见 `arch/terminal-emulator.md`。

#![warn(missing_docs)]

pub mod domain;
pub mod error;
pub mod platform;

pub use error::{Error, Result};
