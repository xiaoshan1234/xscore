# Task 12: CLI `config` subcommand

Same shape as before (AppConfig JSON).

**Files:**
- Modify: `crates/xscore/src/cli/config.rs`
- Modify: `crates/xscore/src/model/config.rs`

```rust
// model/config.rs
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    #[serde(default)]
    pub defaults: HashMap<String, serde_json::Value>,
    #[serde(default)]
    pub tmux_socket: Option<String>,
}
```

```rust
// cli/config.rs
use crate::config;
use crate::model::AppConfig;
use clap::Subcommand;

#[derive(Subcommand)]
pub enum ConfigCmd {
    Show,
    Path,
    Set { key: String, value: String },
    Unset { key: String },
}

pub async fn run(cmd: ConfigCmd) -> anyhow::Result<()> {
    config::ensure_dirs()?;
    match cmd {
        ConfigCmd::Show => {
            let cfg: AppConfig = if config::config_file().exists() {
                serde_json::from_str(&std::fs::read_to_string(config::config_file())?)?
            } else { AppConfig::default() };
            println!("{}", serde_json::to_string_pretty(&cfg)?);
        }
        ConfigCmd::Path => {
            println!("config: {}", config::config_file().display());
            println!("sessions: {}", config::sessions_dir().display());
        }
        ConfigCmd::Set { key, value } => {
            let mut cfg = load_or_default();
            apply_set(&mut cfg, &key, &value);
            save(&cfg)?;
            println!("set {key} = {value}");
        }
        ConfigCmd::Unset { key } => {
            let mut cfg = load_or_default();
            apply_unset(&mut cfg, &key);
            save(&cfg)?;
            println!("unset {key}");
        }
    }
    Ok(())
}

fn load_or_default() -> AppConfig {
    if config::config_file().exists() {
        serde_json::from_str(&std::fs::read_to_string(config::config_file()).unwrap_or_default())
            .unwrap_or_default()
    } else { AppConfig::default() }
}

fn save(cfg: &AppConfig) -> anyhow::Result<()> {
    config::ensure_dirs()?;
    std::fs::write(config::config_file(), serde_json::to_string_pretty(cfg)?)?;
    Ok(())
}

fn apply_set(cfg: &mut AppConfig, key: &str, value: &str) {
    match key {
        "tmux_socket" => cfg.tmux_socket = Some(value.into()),
        _ => { cfg.defaults.insert(key.into(), serde_json::Value::String(value.into())); }
    }
}

fn apply_unset(cfg: &mut AppConfig, key: &str) {
    if key == "tmux_socket" { cfg.tmux_socket = None; }
    else { cfg.defaults.remove(key); }
}
```

**Step 4: Commit**

```bash
git add crates/xscore/src/cli/config.rs crates/xscore/src/model/config.rs
git commit -m "feat(cli): config show/path/set/unset over AppConfig JSON"
```