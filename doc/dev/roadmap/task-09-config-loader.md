# Task 9: Config loader

Save/load `ConnectSpec` JSON to `~/.config/xscore/sessions/link-<link-id>.json`.

**Files:**
- Create: `crates/xscore/src/config/{mod,paths,loader}.rs`

```rust
// config/paths.rs
pub const APP_DIR: &str = "xscore";
pub fn config_dir() -> std::path::PathBuf {
    dirs::config_dir().unwrap_or_else(|| std::path::PathBuf::from(".config")).join(APP_DIR)
}
pub fn sessions_dir() -> std::path::PathBuf { config_dir().join("sessions") }
pub fn config_file() -> std::path::PathBuf { config_dir().join("config.json") }
pub fn ensure_dirs() -> std::io::Result<()> {
    std::fs::create_dir_all(config_dir())?;
    std::fs::create_dir_all(sessions_dir())?;
    Ok(())
}
pub fn link_path(link_id: &str) -> std::path::PathBuf { sessions_dir().join(format!("link-{link_id}.json")) }
```

```rust
// config/loader.rs
use crate::core::Result;
use crate::model::ConnectSpec;
use std::path::Path;

pub fn save_connect_spec(path: &Path, spec: &ConnectSpec) -> Result<()> {
    if let Some(parent) = path.parent() { std::fs::create_dir_all(parent)?; }
    std::fs::write(path, serde_json::to_string_pretty(spec)?)?;
    Ok(())
}
pub fn load_connect_spec(path: &Path) -> Result<ConnectSpec> {
    Ok(serde_json::from_str(&std::fs::read_to_string(path)?)?)
}
```

```rust
// config/mod.rs
pub mod loader;
pub mod paths;

pub use loader::{load_connect_spec, save_connect_spec};
pub use paths::{config_dir, config_file, ensure_dirs, link_path, sessions_dir};
```

**Step 1: Commit**

```bash
git add crates/xscore/src/config/
git commit -m "feat(config): XDG paths + ConnectSpec save/load"
```