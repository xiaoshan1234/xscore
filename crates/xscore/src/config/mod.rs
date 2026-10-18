// config/mod.rs
pub mod loader;
pub mod paths;

pub use loader::{load_connect_spec, save_connect_spec, list_link_configs};
pub use paths::{config_dir, ensure_dirs, link_config_path, links_dir, state_dir, state_link_path};