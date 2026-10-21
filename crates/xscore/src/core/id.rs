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
                Ok(Self(Uuid::parse_str(s)?))
            }
        }

        impl Default for $name { fn default() -> Self { Self::new() } }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { self.0.fmt(f) }
        }
    };
}

uuid_newtype!(SessionId);
uuid_newtype!(LinkId);
uuid_newtype!(WindowId);
uuid_newtype!(PaneId);

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
    fn window_id_roundtrip_via_string() {
        let id = WindowId::new();
        let s = id.to_string();
        let parsed = WindowId::parse(&s).unwrap();
        assert_eq!(id, parsed);
    }

    #[test]
    fn session_id_roundtrip_via_string() {
        let id = SessionId::new();
        let s = id.to_string();
        let parsed = SessionId::parse(&s).unwrap();
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
