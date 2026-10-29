//! 终端模拟器后端（L0–L2）。
//!
//! 分层严格单向：`platform`（L0）← `domain`（L1）← `term`（L2 纯逻辑）。
//! 权威架构见 `doc/arch/terminal-emulator.md`。

pub mod domain;
pub mod error;
pub mod platform;
pub mod term;

pub use error::{Error, Result};
