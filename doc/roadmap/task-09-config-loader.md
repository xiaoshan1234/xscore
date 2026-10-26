# Task 9: Config loader

Save/load `ConnectSpec` JSON. New layout uses `~/.config/xscore/links/<name>.json` for persistent templates (Task 10's `link config ...` group). The old `sessions/` directory is removed; `state/links/<id>.json` is reserved for the future persistent-link-state feature but **not used in this slice**.

**Files:**
- Create: `crates/xscore/src/config/{mod,paths,loader}.rs`

```rust
// config/paths.rs
pub const APP_DIR: &str = "xscore";

pub fn config_dir() -> std::path::PathBuf {
    dirs::config_dir().unwrap_or_else(|| std::path::PathBuf::from(".config")).join(APP_DIR)
}
pub fn config_file() -> std::path::PathBuf { config_dir().join("config.json") }

pub fn links_dir() -> std::path::PathBuf { config_dir().join("links") }
pub fn link_config_path(name: &str) -> std::path::PathBuf { links_dir().join(format!("{name}.json")) }

pub fn state_dir() -> std::path::PathBuf { config_dir().join("state") }
pub fn state_link_path(id: &str) -> std::path::PathBuf { state_dir().join("links").join(format!("{id}.json")) }

pub fn ensure_dirs() -> std::io::Result<()> {
    std::fs::create_dir_all(config_dir())?;
    std::fs::create_dir_all(links_dir())?;
    Ok(())
}
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

/// Iterate all ConnectSpec templates under `links/`. Sorted by path for stable output.
pub fn list_link_configs() -> Result<Vec<std::path::PathBuf>> {
    let dir = super::paths::links_dir();
    if !dir.exists() { return Ok(vec![]); }
    let mut out = vec![];
    for entry in std::fs::read_dir(dir)? {
        let e = entry?;
        let p = e.path();
        if p.extension().and_then(|x| x.to_str()) == Some("json") {
            out.push(p);
        }
    }
    out.sort();
    Ok(out)
}
```

```rust
// config/mod.rs
pub mod loader;
pub mod paths;

pub use loader::{list_link_configs, load_connect_spec, save_connect_spec};
pub use paths::{
    config_dir, config_file, ensure_dirs, link_config_path, links_dir,
    state_dir, state_link_path,
};
```

**Step 1: Commit**

```bash
git add crates/xscore/src/config/
git commit -m "feat(config): XDG paths + ConnectSpec save/load (links/ for templates)"
```