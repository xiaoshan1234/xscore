//! Reusable terminal emulator server state, command handling, and daemon runtime.

mod cli;

pub mod app;
pub mod commands;
pub mod layout;
pub mod logging;
pub mod render;
pub mod server;
pub mod term;
pub mod tmux;
