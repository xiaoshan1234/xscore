//! SessionService-owned persisted model.

use serde::{Deserialize, Serialize};

use crate::infra::tmux::{TmuxHostConfig, TmuxSocket};

uuid_id!(SessionId);

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SessionInfo {
    pub id: SessionId,
    pub name: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub backend: SessionBackend,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SessionBackend {
    Native,
    Tmux {
        host: TmuxHostConfig,
        socket: TmuxSocket,
        session: String,
    },
}

impl SessionInfo {
    pub fn new_native(name: impl Into<String>) -> Self {
        Self {
            id: SessionId::new(),
            name: name.into(),
            created_at: chrono::Utc::now(),
            backend: SessionBackend::Native,
        }
    }

    pub fn new_tmux(
        name: impl Into<String>,
        host: TmuxHostConfig,
        socket: TmuxSocket,
        session: impl Into<String>,
    ) -> Self {
        Self {
            id: SessionId::new(),
            name: name.into(),
            created_at: chrono::Utc::now(),
            backend: SessionBackend::Tmux {
                host,
                socket,
                session: session.into(),
            },
        }
    }

    pub fn is_tmux(&self) -> bool {
        matches!(self.backend, SessionBackend::Tmux { .. })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_info_roundtrips_with_its_children() {
        let info = SessionInfo::new_tmux(
            "work",
            TmuxHostConfig::Ssh {
                destination: "devbox".into(),
                args: vec![],
            },
            TmuxSocket::Default,
            "tmux-work",
        );
        assert!(info.is_tmux());
        let json = serde_json::to_string(&info).unwrap();
        let back: SessionInfo = serde_json::from_str(&json).unwrap();
        assert_eq!(back, info);
    }

    #[test]
    fn native_backend_is_tagged_object() {
        let json = serde_json::to_string(&SessionBackend::Native).unwrap();
        assert_eq!(json, r#"{"kind":"native"}"#);
    }

    #[test]
    fn session_id_roundtrip_via_string() {
        let id = SessionId::new();
        assert_eq!(SessionId::parse(&id.to_string()).unwrap(), id);
    }
}
