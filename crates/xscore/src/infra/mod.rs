//! Concrete I/O adapters. Nothing above this layer opens a socket, PTY,
//! serial device or tmux control client directly.

pub mod config;
pub mod link;
pub mod tmux;
pub mod url;
pub mod util;
