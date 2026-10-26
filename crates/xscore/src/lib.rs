//! Layered xscore architecture.
//!
//! `app -> ui/service -> domain`, while `service -> infra` supplies concrete
//! I/O. The domain kernel has no dependency on the outer layers.

pub mod app;
pub mod domain;
pub mod infra;
pub mod service;
pub mod ui;
