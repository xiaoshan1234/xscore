# Task 2: Core IDs + error type + LinkAddress

**Objective:** Define `LinkId` (uuid v7), `PaneId` (uuid v7), `LinkAddress` (hashable transport identity), `core::Error`.

**Files:**
- Create: `crates/xscore/src/core/mod.rs`
- Create: `crates/xscore/src/core/id.rs`
- Create: `crates/xscore/src/core/address.rs`
- Create: `crates/xscore/src/core/error.rs`
- Create: `crates/xscore/src/core/{manager,link,pane,link_registry,pane_registry}.rs` (placeholders)

**Step 1: Write failing tests** (`id.rs` top)

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn link_id_roundtrip_via_string() {
        let id = LinkId::new();
        let s = id.to_string();
        let parsed = LinkId::parse(&s).unwrap();
        assert_eq!(id, parsed);
    }

    #[test]
    fn pane_id_roundtrip_via_string() {
        let id = PaneId::new();
        let s = id.to_string();
        let parsed = PaneId::parse(&s).unwrap();
        assert_eq!(id, parsed);
    }

    #[test]
    fn link_and_pane_ids_distinct_types() {
        // LinkId and PaneId share string form but are distinct newtypes.
        let lid = LinkId::parse("01928e0a-7d10-7000-8000-000000000000").unwrap();
        let pid = PaneId::parse("01928e0a-7d10-7000-8000-000000000001").unwrap();
        // cannot compare directly (different types); just exercise parse.
        let _ = (lid, pid);
    }
}
```

**Step 2: Implement IDs**

```rust
// core/id.rs
use serde::{Deserialize, Serialize};
use std::fmt;
use uuid::Uuid;

macro_rules! uuid_newtype {
    ($name:ident) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(Uuid);

        impl $name {
            pub fn new() -> Self { Self(Uuid::now_v7()) }
            pub fn as_uuid(&self) -> Uuid { self.0 }
            pub fn parse(s: &str) -> Result<Self, uuid::Error> {
                Ok(Self(Uuid::parse(s)?))
            }
        }

        impl Default for $name { fn default() -> Self { Self::new() } }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { self.0.fmt(f) }
        }
    };
}

uuid_newtype!(LinkId);
uuid_newtype!(PaneId);
```

**Step 3: Implement `LinkAddress`** (`core/address.rs`)

```rust
// core/address.rs
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Hash, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "transport", rename_all = "snake_case")]
pub enum LinkAddress {
    Shell { command: String, args: Vec<String> },
    Serial { path: String },
    Ssh { host: String, port: u16, user: String, identity: Option<String>, proxy: Option<String> },
}

impl LinkAddress {
    pub fn kind(&self) -> &'static str {
        match self {
            LinkAddress::Shell { .. } => "shell",
            LinkAddress::Serial { .. } => "serial",
            LinkAddress::Ssh { .. } => "ssh",
        }
    }
}
```

**Step 4: Failing test for LinkAddress**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ssh_address_equal_when_full_tuple_matches() {
        let a = LinkAddress::Ssh {
            host: "h".into(), port: 22, user: "u".into(),
            identity: Some("id".into()), proxy: None,
        };
        let b = LinkAddress::Ssh {
            host: "h".into(), port: 22, user: "u".into(),
            identity: Some("id".into()), proxy: None,
        };
        assert_eq!(a, b);
        let mut h = std::collections::HashSet::new();
        h.insert(a.clone()); h.insert(b.clone());
        assert_eq!(h.len(), 1);
    }

    #[test]
    fn ssh_address_different_when_port_differs() {
        let a = LinkAddress::Ssh {
            host: "h".into(), port: 22, user: "u".into(),
            identity: None, proxy: None,
        };
        let b = LinkAddress::Ssh {
            host: "h".into(), port: 2222, user: "u".into(),
            identity: None, proxy: None,
        };
        assert_ne!(a, b);
    }
}
```

**Step 5: Implement `core::Error`**

```rust
// core/error.rs
use thiserror::Error;
use super::id::{LinkId, PaneId};

#[derive(Debug, Error)]
pub enum Error {
    #[error("link not found: {0}")]
    LinkNotFound(LinkId),

    #[error("pane not found: {0}")]
    PaneNotFound(PaneId),

    #[error("--link=existing not supported for transport: {0}")]
    LinkExistingUnsupported(&'static str),

    #[error("url parse error: {0}")]
    Url(String),

    #[error("transport error: {0}")]
    Transport(String),

    #[error("sink error: {0}")]
    Sink(String),

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("config error: {0}")]
    Config(String),

    #[error("invalid input: {0}")]
    Invalid(String),

    #[error("serialization error: {0}")]
    Serde(#[from] serde_json::Error),
}

pub type Result<T> = std::result::Result<T, Error>;
```

**Step 6: `core/mod.rs`**

```rust
pub mod address;
pub mod error;
pub mod id;
pub mod link;
pub mod link_registry;
pub mod manager;
pub mod pane;
pub mod pane_registry;

pub use address::LinkAddress;
pub use error::{Error, Result};
pub use id::{LinkId, PaneId};
```

**Step 7: Run tests**

Run: `cargo test -p xscore core::id core::address`
Expected: 4 passed.

**Step 8: Commit**

```bash
git add crates/xscore/src/core/
git commit -m "feat(core): LinkId/PaneId (uuid v7) + LinkAddress + Error"
```