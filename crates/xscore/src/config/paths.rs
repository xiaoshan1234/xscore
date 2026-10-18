// config/paths.rs
use std::path::PathBuf;

pub fn config_dir() -> PathBuf {
    // Standard XDG-style location for per-user config: `~/.config/xscore/`.
    // Fall back to `./.xscore/` if HOME is unset (e.g. running in a container).
    if let Some(home) = std::env::var_os("HOME") {
        PathBuf::from(home).join(".config").join("xscore")
    } else {
        PathBuf::from(".xscore")
    }
}

pub fn links_dir() -> PathBuf { config_dir().join("links") }
pub fn link_config_path(name: &str) -> PathBuf { links_dir().join(format!("{name}.json")) }

pub fn state_dir() -> PathBuf { config_dir().join("state") }
pub fn state_link_path(id: &str) -> PathBuf { state_dir().join("links").join(format!("{id}.json")) }

pub fn ensure_dirs() -> std::io::Result<()> {
    std::fs::create_dir_all(config_dir())?;
    std::fs::create_dir_all(links_dir())?;
    std::fs::create_dir_all(state_dir())?;
    Ok(())
}

/// `~/.config/xscore/state/state.json` — single-file snapshot of the
/// in-memory link metadata (ADR 0006 / task-15). Windows + panes are
/// re-discovered from tmux on hydration, not persisted.
pub fn state_snapshot_path() -> std::path::PathBuf { state_dir().join("state.json") }