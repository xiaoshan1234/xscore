# Task 3: URL → ConnectSpec + LinkAddress

**Objective:** Pure parser for `xscore link add <url>` → `(ConnectSpec, LinkAddress)`.

**Files:**
- Create: `crates/xscore/src/model/session.rs`
- Create: `crates/xscore/src/model/mod.rs`
- Create: `crates/xscore/src/url/mod.rs`
- Create: `crates/xscore/src/url/scheme.rs` (placeholder)
- Create: `crates/xscore/src/url/connect.rs`

**Step 1: Define config types** (`model/session.rs`)

```rust
// model/session.rs
use serde::{Deserialize, Serialize};
use crate::core::{LinkAddress, LinkId, PaneId};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "transport", rename_all = "snake_case")]
pub enum TransportConfig {
    Shell { command: String, #[serde(default)] args: Vec<String>, #[serde(default)] cwd: Option<String> },
    Serial { path: String, baud: u32, #[serde(default = "default_data_bits")] data_bits: u8,
             #[serde(default = "default_parity")] parity: String, #[serde(default = "default_stop_bits")] stop_bits: u8 },
    Ssh { host: String, #[serde(default = "default_ssh_port")] port: u16, user: String,
          #[serde(default)] identity: Option<String>, #[serde(default)] proxy: Option<String> },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "sink", rename_all = "snake_case")]
pub enum SinkConfig {
    Tmux {
        #[serde(default = "default_tmux_mode")]
        mode: String,
        #[serde(default)]
        session_name: Option<String>,
    },
}

fn default_ssh_port() -> u16 { 22 }
fn default_data_bits() -> u8 { 8 }
fn default_parity() -> String { "n".into() }
fn default_stop_bits() -> u8 { 1 }
fn default_tmux_mode() -> String { "auto".into() }

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ConnectSpec {
    #[serde(default = "default_name")]
    pub name: String,
    #[serde(flatten)]
    pub transport: TransportConfig,
    pub sink: SinkConfig,
}

fn default_name() -> String { uuid::Uuid::now_v7().to_string() }

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct LinkInfo {
    pub id: LinkId,
    pub name: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub address: LinkAddress,
    pub transport: TransportConfig,
    pub sink: SinkConfig,
}

impl LinkInfo {
    pub fn transport_str(&self) -> &'static str { self.address.kind() }
    pub fn sink_str(&self) -> &'static str {
        match &self.sink {
            SinkConfig::Tmux { .. } => "tmux",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PaneInfo {
    pub id: PaneId,
    pub link_id: LinkId,
    pub name: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub sink_handle: String,    // e.g. "%42"
    pub pane_index: u32,        // 0-based, set by sink
}
```

**Step 2: Write failing tests**

```rust
// url/connect.rs (test)
#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{SinkConfig, TransportConfig};
    use crate::core::LinkAddress;

    #[test]
    fn parse_ssh_url_with_tmux() {
        let (spec, addr) = parse("ssh://root@10.0.0.1:22?identity=~/.ssh/id_ed25519").unwrap();
        assert!(matches!(spec.transport, TransportConfig::Ssh { .. }));
        assert_eq!(addr, LinkAddress::Ssh {
            host: "10.0.0.1".into(), port: 22, user: "root".into(),
            identity: Some("~/.ssh/id_ed25519".into()), proxy: None,
        });
    }

    #[test]
    fn parse_shell_url_yields_shell_address() {
        let (_, addr) = parse("shell:///bin/bash").unwrap();
        assert_eq!(addr, LinkAddress::Shell { command: "/bin/bash".into(), args: vec![] });
    }

    #[test]
    fn parse_serial_url_yields_serial_address() {
        let (_, addr) = parse("serial:///dev/ttyUSB0?baud=115200").unwrap();
        assert_eq!(addr, LinkAddress::Serial { path: "/dev/ttyUSB0".into() });
    }

    #[test]
    fn parse_rejects_unknown_scheme() {
        assert!(parse("telnet://host").is_err());
    }

    #[test]
    fn ssh_with_different_identity_different_address() {
        let (_, a1) = parse("ssh://u@h:22?identity=k1").unwrap();
        let (_, a2) = parse("ssh://u@h:22?identity=k2").unwrap();
        assert_ne!(a1, a2);
    }
}
```

**Step 3: Implement parser**

```rust
// url/connect.rs
use crate::core::{Error, LinkAddress, Result};
use crate::model::{ConnectSpec, SinkConfig, TransportConfig};
use std::collections::HashMap;

pub fn parse(s: &str) -> Result<(ConnectSpec, LinkAddress)> {
    let url = url::Url::parse(s).map_err(|e| Error::Url(e.to_string()))?;
    let scheme = url.scheme();
    let host = url.host_str().map(|s| s.to_string());
    let user = (!url.username().is_empty()).then(|| url.username().to_string());

    let mut pairs: HashMap<String, String> = url.query_pairs()
        .map(|(k, v)| (k.into_owned(), v.into_owned())).collect();
    let tmux_mode = pairs.remove("tmux").unwrap_or_else(|| "auto".into());
    let name = pairs.remove("name").unwrap_or_default();

    let (transport, address) = match scheme {
        "shell" | "sh" => {
            let command = url.path().trim_start_matches('/').to_string();
            if command.is_empty() { return Err(Error::Url("shell URL needs command".into())); }
            (TransportConfig::Shell { command: command.clone(), args: vec![], cwd: None },
             LinkAddress::Shell { command, args: vec![] })
        }
        "serial" => {
            let path = url.path().to_string();
            if path.is_empty() { return Err(Error::Url("serial URL needs path".into())); }
            let baud: u32 = pairs.remove("baud")
                .ok_or_else(|| Error::Url("serial needs baud".into()))?
                .parse().map_err(|e: std::num::ParseIntError| Error::Url(e.to_string()))?;
            let data_bits: u8 = pairs.remove("data_bits").unwrap_or_else(|| "8".into()).parse()
                .map_err(|e: std::num::ParseIntError| Error::Url(e.to_string()))?;
            let parity = pairs.remove("parity").unwrap_or_else(|| "n".into());
            let stop_bits: u8 = pairs.remove("stop_bits").unwrap_or_else(|| "1".into()).parse()
                .map_err(|e: std::num::ParseIntError| Error::Url(e.to_string()))?;
            (TransportConfig::Serial { path: path.clone(), baud, data_bits, parity, stop_bits },
             LinkAddress::Serial { path })
        }
        "ssh" => {
            let host = host.ok_or_else(|| Error::Url("ssh URL needs host".into()))?;
            let user = user.ok_or_else(|| Error::Url("ssh URL needs user".into()))?;
            let port = url.port().unwrap_or(22);
            let identity = pairs.remove("identity");
            let proxy = pairs.remove("proxy");
            (TransportConfig::Ssh { host: host.clone(), port, user: user.clone(),
                                   identity: identity.clone(), proxy: proxy.clone() },
             LinkAddress::Ssh { host, port, user, identity, proxy })
        }
        other => return Err(Error::Url(format!("unknown scheme: {other}"))),
    };

    let sink = SinkConfig::Tmux {
        mode: tmux_mode,
        session_name: (!name.is_empty()).then(|| name.clone()),
    };

    Ok((ConnectSpec { name, transport, sink }, address))
}
```

```rust
// url/mod.rs
pub mod connect;
pub mod scheme;

pub use connect::parse;
```

**Step 4: Update `model/mod.rs`**

```rust
pub mod session;
pub mod config;       // placeholder

pub use session::{ConnectSpec, LinkInfo, PaneInfo, SinkConfig, TransportConfig};
```

**Step 5: Run tests**

Run: `cargo test -p xscore url`
Expected: 5 passing.

**Step 6: Commit**

```bash
git add crates/xscore/src/url/ crates/xscore/src/model/
git commit -m "feat(url): parse connect URL into (ConnectSpec, LinkAddress)"
```