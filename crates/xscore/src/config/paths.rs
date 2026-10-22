// config/paths.rs
use std::path::PathBuf;

pub fn config_dir() -> PathBuf {
    // Keep one state layout on Linux, WSL and native Windows. HOME wins so
    // tests and containers can isolate state; Windows falls back to the user
    // profile when a POSIX-style HOME is not present.
    let home = std::env::var_os("HOME")
        .filter(|value| !value.is_empty())
        .or_else(|| {
            #[cfg(windows)]
            {
                std::env::var_os("USERPROFILE").filter(|value| !value.is_empty())
            }
            #[cfg(not(windows))]
            {
                None
            }
        });
    home.map(|home| PathBuf::from(home).join(".config").join("xscore"))
        .unwrap_or_else(|| PathBuf::from(".xscore"))
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
