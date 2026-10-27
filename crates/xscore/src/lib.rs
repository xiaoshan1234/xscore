//! Layered xscore architecture.
//!
//! `app -> service -> ui/infra`. Persisted models live with the module that
//! owns them; only `Error`/`Result` and the ID macro are crate-level utilities.

mod error;
#[macro_use]
mod macros;

pub mod app;
pub mod daemon;
pub mod infra;
pub mod service;
pub mod ui;

pub use error::{Error, Result};
