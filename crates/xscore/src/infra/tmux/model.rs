//! tmux backend target model.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TmuxHostConfig {
    #[default]
    Local,
    Ssh {
        destination: String,
        args: Vec<String>,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub enum TmuxSocket {
    Default,
    Named(String),
    Path(String),
}

#[derive(Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum TmuxSocketWire<'a> {
    Default,
    Named { name: &'a str },
    Path { path: &'a str },
}

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum TmuxSocketRepr {
    Default,
    Named { name: String },
    Path { path: String },
}

impl Serialize for TmuxSocket {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let wire = match self {
            TmuxSocket::Default => TmuxSocketWire::Default,
            TmuxSocket::Named(name) => TmuxSocketWire::Named { name },
            TmuxSocket::Path(path) => TmuxSocketWire::Path { path },
        };
        wire.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for TmuxSocket {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Ok(match TmuxSocketRepr::deserialize(deserializer)? {
            TmuxSocketRepr::Default => TmuxSocket::Default,
            TmuxSocketRepr::Named { name } => TmuxSocket::Named(name),
            TmuxSocketRepr::Path { path } => TmuxSocket::Path(path),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn socket_shapes_roundtrip() {
        for socket in [
            TmuxSocket::Default,
            TmuxSocket::Named("x".into()),
            TmuxSocket::Path("/tmp/s".into()),
        ] {
            let json = serde_json::to_string(&socket).unwrap();
            let back: TmuxSocket = serde_json::from_str(&json).unwrap();
            assert_eq!(back, socket);
        }
    }
}
