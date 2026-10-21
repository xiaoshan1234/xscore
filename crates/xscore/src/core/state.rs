// core/state.rs (v0.4.0 — schema_version = 2, ADR 0008)
//
// v0.3.0 made the snapshot FLAT: three parallel lists (links, windows,
// panes) — the entities are independent and the associations are stored
// ON the panes (pane.info.link_id + window_id), not on a snapshot
// wrapper grouping them by link.
//
// v0.4.0 adds an explicit `schema_version` and refuses to load anything
// that isn't exactly `SCHEMA_VERSION`. Rationale (P0-e): before v0.4.0
// a stale file was silently deserialized into a half-empty snapshot
// (serde filled the missing fields), so the user's links simply
// "disappeared" with no diagnostic. Explicit failure is always better
// than silent data loss. Users upgrading from v0.3.0 must delete
// `~/.config/xscore/state/state.json`.
//
// `schema_version` deliberately has NO serde default: a missing field
// must not be able to masquerade as the current version. The loader
// reads it as a probe first (treating "missing" as 0) so the error
// message can be specific.

use std::path::Path;

use crate::core::{Error, Result};
use crate::core::PaneId;
use crate::model::{LinkInfo, PaneInfo, WindowInfo};

/// Current on-disk schema version. Bump on any breaking shape change.
pub const SCHEMA_VERSION: u32 = 2;

/// v0.4.0 flat snapshot: three parallel lists, no grouping.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct StateSnapshot {
    /// Must equal `SCHEMA_VERSION`. No serde default on purpose.
    pub schema_version: u32,
    pub links: Vec<LinkInfo>,
    pub windows: Vec<WindowSnapshot>,
    pub panes: Vec<PaneInfo>,
}

impl Default for StateSnapshot {
    fn default() -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            links: Vec::new(),
            windows: Vec::new(),
            panes: Vec::new(),
        }
    }
}

/// One window's worth of persisted state. Flattened `WindowInfo` + the
/// list of pane ids that currently live in this window. (Pane ids only —
/// the pane's full `PaneInfo` lives in `StateSnapshot.panes`.)
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct WindowSnapshot {
    #[serde(flatten)]
    pub info: WindowInfo,
    pub panes: Vec<PaneId>,
}

/// Persist a snapshot to disk. Call after any mutating manager method
/// (add_link, add_pane, attach_pane, etc.) so the next CLI invocation
/// sees the change. Crash-safe: writes to `state.json.tmp` then renames
/// atomically.
pub fn save(snapshot: &StateSnapshot) -> Result<()> {
    let path = super::super::config::paths::state_snapshot_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| Error::Config(format!("mkdir {}: {e}", parent.display())))?;
    }
    let bytes = serde_json::to_vec_pretty(snapshot)?;
    // Per-process temp name: two concurrent CLI invocations both saving
    // used to race on a single `state.json.tmp`, and the loser's rename
    // failed with ENOENT *after* the winner had consumed the file (v0.5.0
    // makes concurrent invocations normal — the daemon can be auto-started
    // by several processes at once).
    let tmp = path.with_extension(format!("json.{}.tmp", std::process::id()));
    std::fs::write(&tmp, &bytes)
        .map_err(|e| Error::Config(format!("write {}: {e}", tmp.display())))?;
    std::fs::rename(&tmp, &path)
        .map_err(|e| Error::Config(format!("rename {}: {e}", path.display())))?;
    Ok(())
}

/// Load a snapshot from disk. Returns `Ok(None)` if no snapshot file
/// exists (first run, or after the user cleared state).
pub fn load() -> Result<Option<StateSnapshot>> {
    let path = super::super::config::paths::state_snapshot_path();
    load_from(&path)
}

fn load_from(path: &Path) -> Result<Option<StateSnapshot>> {
    if !path.exists() {
        return Ok(None);
    }
    let bytes = std::fs::read(path)
        .map_err(|e| Error::Config(format!("read {}: {e}", path.display())))?;
    parse_snapshot(&bytes, path)
}

/// Parse + validate a snapshot. Split out from `load_from` so the
/// version gate is unit-testable without touching `$HOME`.
pub fn parse_snapshot(bytes: &[u8], path: &Path) -> Result<Option<StateSnapshot>> {
    if bytes.is_empty() {
        return Ok(None);
    }
    // Probe the version first: a missing/incompatible `schema_version`
    // must fail loudly instead of deserializing into a half-empty state.
    let probe: serde_json::Value = serde_json::from_slice(bytes)
        .map_err(|e| Error::Config(format!("parse {}: {e}", path.display())))?;
    let found = probe.get("schema_version").and_then(|v| v.as_u64()).unwrap_or(0);
    if found != SCHEMA_VERSION as u64 {
        return Err(Error::Config(format!(
            "state.json schema_version={found}, expected {SCHEMA_VERSION}; \
             delete ~/.config/xscore/state/state.json to reset"
        )));
    }
    let snapshot: StateSnapshot = serde_json::from_slice(bytes)
        .map_err(|e| Error::Config(format!("parse {}: {e}", path.display())))?;
    Ok(Some(snapshot))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn path() -> PathBuf { PathBuf::from("/tmp/does-not-matter.json") }

    #[test]
    fn empty_bytes_is_none() {
        assert!(parse_snapshot(b"", &path()).unwrap().is_none());
    }

    #[test]
    fn missing_schema_version_is_rejected() {
        // v0.3.0 shape — three lists, no version field.
        let legacy = br#"{"links":[],"windows":[],"panes":[]}"#;
        let err = parse_snapshot(legacy, &path()).unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("schema_version=0"), "got: {msg}");
        assert!(msg.contains("expected 2"), "got: {msg}");
        assert!(msg.contains("delete"), "got: {msg}");
    }

    #[test]
    fn wrong_schema_version_is_rejected() {
        let bad = br#"{"schema_version":1,"links":[],"windows":[],"panes":[]}"#;
        let err = parse_snapshot(bad, &path()).unwrap_err();
        assert!(format!("{err}").contains("schema_version=1"), "{err}");
    }

    #[test]
    fn current_schema_version_roundtrips() {
        let snap = StateSnapshot::default();
        let bytes = serde_json::to_vec(&snap).unwrap();
        let parsed = parse_snapshot(&bytes, &path()).unwrap().unwrap();
        assert_eq!(parsed.schema_version, SCHEMA_VERSION);
    }
}
