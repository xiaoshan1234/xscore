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
