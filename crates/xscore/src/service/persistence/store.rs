use super::snapshot::*;
use crate::{Error, Result};
use std::path::Path;

pub(super) fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| Error::Config(format!("mkdir {}: {e}", parent.display())))?;
    }
    let tmp = path.with_extension(format!("json.{}.tmp", std::process::id()));
    std::fs::write(&tmp, bytes)
        .map_err(|e| Error::Config(format!("write {}: {e}", tmp.display())))?;
    std::fs::rename(&tmp, path)
        .map_err(|e| Error::Config(format!("rename {}: {e}", path.display())))?;
    Ok(())
}

/// Persist a snapshot to disk. Call after any mutating manager method
/// (add_link, add_pane, attach_pane, etc.) so the next CLI invocation
/// sees the change. Crash-safe: writes to `state.json.tmp` then renames
/// atomically.
pub fn save(snapshot: &StateSnapshot) -> Result<()> {
    let path = crate::infra::config::paths::state_snapshot_path();
    let bytes = serde_json::to_vec_pretty(snapshot)?;
    write_atomic(&path, &bytes)
}

/// Load a snapshot from disk. Returns `Ok(None)` if no snapshot file
/// exists (first run, or after the user cleared state).
pub fn load() -> Result<Option<StateSnapshot>> {
    let path = crate::infra::config::paths::state_snapshot_path();
    load_from(&path)
}

fn load_from(path: &Path) -> Result<Option<StateSnapshot>> {
    if !path.exists() {
        return Ok(None);
    }
    let bytes =
        std::fs::read(path).map_err(|e| Error::Config(format!("read {}: {e}", path.display())))?;
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
    let found = probe
        .get("schema_version")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    if found != SCHEMA_VERSION as u64 {
        return Err(Error::Config(format!(
            "state.json schema_version={found}, expected {SCHEMA_VERSION}; \
             run `xscore state migrate` to convert a v2 file, or \
             delete ~/.config/xscore/state/state.json to reset"
        )));
    }
    // v0.6.0 PR4: a v3 file written before the control client became the only
    // tmux path still describes links, windows and panes with the fields that
    // path wrote. The version gate cannot see that, and deserializing it would
    // quietly lose every tmux session the user had — so name the one command
    // that converts it.
    if legacy_rows(&probe) {
        return Err(Error::Config(format!(
            "state.json at {} was written by an older v3 build (its rows still carry \
             the deleted tmux path's fields); run `xscore state migrate` to convert it",
            path.display()
        )));
    }
    let snapshot: StateSnapshot = serde_json::from_slice(bytes)
        .map_err(|e| Error::Config(format!("parse {}: {e}", path.display())))?;
    Ok(Some(snapshot))
}

/// Does any row still carry a field that only the deleted tmux path wrote?
fn legacy_rows(root: &serde_json::Value) -> bool {
    let any = |key: &str, list: &str| {
        root.get(list)
            .and_then(|v| v.as_array())
            .map(|rows| rows.iter().any(|row| row.get(key).is_some()))
            .unwrap_or(false)
    };
    any("sink", "links")
        || any("tmux_window_handle", "windows")
        || any("layout", "windows")
        || any("sink_handle", "panes")
        || any("link_id", "panes")
        || any("link_key", "panes")
}

/// The schema version on disk, without parsing the whole file. `0` when the
/// field is missing — the same probe `parse_snapshot` uses.
pub fn on_disk_version() -> Result<u32> {
    let path = crate::infra::config::paths::state_snapshot_path();
    if !path.exists() {
        return Err(Error::Config(format!(
            "no state file at {}; nothing to migrate",
            path.display()
        )));
    }
    let bytes =
        std::fs::read(&path).map_err(|e| Error::Config(format!("read {}: {e}", path.display())))?;
    let root: serde_json::Value = serde_json::from_slice(&bytes)
        .map_err(|e| Error::Config(format!("parse {}: {e}", path.display())))?;
    Ok(root
        .get("schema_version")
        .and_then(|v| v.as_u64())
        .unwrap_or(0) as u32)
}
