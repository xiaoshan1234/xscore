// core/state.rs (v0.3.0 — flat schema, ADR 0007 v2)
//
// v0.3.0 schema redesign. Pre-v0.3.0 the snapshot was `Vec<LinkSnapshot>`
// where each snapshot grouped one link's `windows: Vec<WindowInfo>` and
// `panes: Vec<PaneInfo>` under the link. v0.3.0 is FLAT: three parallel
// lists (links, windows, panes) — the entities are independent and the
// associations are now stored ON the panes (pane.info.link_id + window_id),
// not on the snapshot wrapper.
//
// Why flat?
//   - Windows are no longer link-scoped (a window can exist without any
//     link, or move between panes that span multiple links in the future).
//   - Panes are orphan-creatable and switchable between links (Q1+Q2 from
//     ADR 0007 v2); the `link_id`/`window_id` references live ON the pane.
//   - Save/load is just `Vec<LinkInfo>` + `Vec<WindowInfo>` +
//     `Vec<PaneInfo>` (plus the panes list on each window).
//
// BREAKING: the v0.2.0 schema is NOT backward-compatible. There is no
// auto-migration. Users must `rm ~/.config/xscore/state/state.json` on
// upgrade — documented in CHANGELOG.

use std::path::Path;

use crate::core::Result;
use crate::core::PaneId;
use crate::model::{LinkInfo, PaneInfo, WindowInfo};

/// v0.3.0 flat snapshot: three parallel lists, no grouping.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, Default)]
pub struct StateSnapshot {
    pub links: Vec<LinkInfo>,
    pub windows: Vec<WindowSnapshot>,
    pub panes: Vec<PaneInfo>,
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
            .map_err(|e| crate::core::Error::Config(format!("mkdir {}: {e}", parent.display())))?;
    }
    let bytes = serde_json::to_vec_pretty(snapshot)?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, &bytes)
        .map_err(|e| crate::core::Error::Config(format!("write {}: {e}", tmp.display())))?;
    std::fs::rename(&tmp, &path)
        .map_err(|e| crate::core::Error::Config(format!("rename {}: {e}", path.display())))?;
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
        .map_err(|e| crate::core::Error::Config(format!("read {}: {e}", path.display())))?;
    if bytes.is_empty() {
        return Ok(None);
    }
    let snapshot: StateSnapshot = serde_json::from_slice(&bytes)
        .map_err(|e| crate::core::Error::Config(format!("parse {}: {e}", path.display())))?;
    Ok(Some(snapshot))
}