// core/state.rs
//
// State persistence for the in-memory registries. The `LinkInfo` is the
// only piece of manager state that's serializable — the `Transport` and
// `Sink` runtime objects are reconstructed on hydration by re-running
// `transport_for(spec.transport)` and `sink_for(...)` against the spec.
//
// On hydration, the manager re-adopts the tmux session (so `kill_window`
// / `kill-session` / `new_pane` work on the right session) and re-registers
// the persisted windows + panes with their ORIGINAL xscore IDs (so
// `xscore window list` after restart shows the same window-id that the
// user saved earlier).
//
// File format: a single `state.json` at `~/.config/xscore/state/state.json`
// containing `Vec<LinkSnapshot>`. Each snapshot holds the LinkInfo plus
// the windows + panes that belong to that link, keyed by xscore's
// uuid-based ids (NOT by tmux ids — those get re-derived on hydration).

use std::path::Path;

use crate::core::Result;
use crate::model::{LinkInfo, PaneInfo, WindowInfo};

/// One link's worth of persisted state. The manager stores these
/// in-memory under `SessionManager` (link, window, pane registries);
/// on save, we walk the registries and group windows + panes by
/// their parent link_id. On load, we re-construct each LinkSnapshot
/// in the same way.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct LinkSnapshot {
    pub link: LinkInfo,
    pub windows: Vec<WindowInfo>,
    pub panes: Vec<PaneInfo>,
}

/// Persist a snapshot of the link metadata to disk. Call after any
/// mutating manager method (add_link, del_link, add_window, etc.) so
/// the next CLI invocation sees the change.
pub fn save(snapshot: &[LinkSnapshot]) -> Result<()> {
    let path = super::super::config::paths::state_snapshot_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| crate::core::Error::Config(format!("mkdir {}: {e}", parent.display())))?;
    }
    let bytes = serde_json::to_vec_pretty(snapshot)?;
    // Atomic write: write to .tmp, then rename. Prevents corruption if
    // the process is killed mid-write.
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, &bytes)
        .map_err(|e| crate::core::Error::Config(format!("write {}: {e}", tmp.display())))?;
    std::fs::rename(&tmp, &path)
        .map_err(|e| crate::core::Error::Config(format!("rename {}: {e}", path.display())))?;
    Ok(())
}

/// Load a snapshot from disk. Returns `Ok(None)` if no snapshot file
/// exists (first run, or after a `del_link` cleared the state).
pub fn load() -> Result<Option<Vec<LinkSnapshot>>> {
    let path = super::super::config::paths::state_snapshot_path();
    load_from(&path)
}

fn load_from(path: &Path) -> Result<Option<Vec<LinkSnapshot>>> {
    if !path.exists() {
        return Ok(None);
    }
    let bytes = std::fs::read(path)
        .map_err(|e| crate::core::Error::Config(format!("read {}: {e}", path.display())))?;
    if bytes.is_empty() {
        return Ok(None);
    }
    let snapshot: Vec<LinkSnapshot> = serde_json::from_slice(&bytes)
        .map_err(|e| crate::core::Error::Config(format!("parse {}: {e}", path.display())))?;
    Ok(Some(snapshot))
}
