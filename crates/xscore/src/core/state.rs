// core/state.rs (v0.6.0 — schema_version = 3, ADR 0010 D8)
//
// v0.3.0 made the snapshot FLAT: three parallel lists (links, windows,
// panes) — the entities are independent and the associations are stored
// ON the panes (pane.info.link_id + window_id), not on a snapshot
// wrapper grouping them by link.
//
// v0.4.0 added an explicit `schema_version` and refuses to load anything
// that isn't exactly `SCHEMA_VERSION`. Rationale (P0-e): before v0.4.0
// a stale file was silently deserialized into a half-empty snapshot
// (serde filled the missing fields), so the user's links simply
// "disappeared" with no diagnostic. Explicit failure is always better
// than silent data loss.
//
// v0.6.0 (ADR 0010 D8) adds the Session tier: a `sessions` list, and every
// link/window/pane carries the `session_id` of the workspace it belongs to.
// The same policy applies — a v2 file is REJECTED, never auto-converted on
// load — but the error now names the one-command way out: `xscore state
// migrate`. `migrate_v2_to_v3` is that way out, and it is the only place in
// this module that reads a file whose version it does not require to be
// current.
//
// `schema_version` deliberately has NO serde default: a missing field
// must not be able to masquerade as the current version. The loader
// reads it as a probe first (treating "missing" as 0) so the error
// message can be specific.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use crate::core::{Error, Result};
use crate::core::{PaneId, SessionId};
use crate::model::{
    LinkInfo, PaneInfo, SessionBackend, SessionInfo, TmuxHostConfig, TmuxSocket, WindowInfo,
};

/// Current on-disk schema version. Bump on any breaking shape change.
pub const SCHEMA_VERSION: u32 = 3;

/// v0.6.0 flat snapshot: the three entity lists plus the sessions that own
/// them. `sessions` comes first on disk so a human reading the file learns the
/// workspace before its children.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct StateSnapshot {
    /// Must equal `SCHEMA_VERSION`. No serde default on purpose.
    pub schema_version: u32,
    pub sessions: Vec<SessionInfo>,
    pub links: Vec<LinkInfo>,
    pub windows: Vec<WindowSnapshot>,
    pub panes: Vec<PaneInfo>,
}

impl Default for StateSnapshot {
    fn default() -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            sessions: Vec::new(),
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

/// Write `bytes` to `path` atomically (tmp file + rename in the same
/// directory). Per-process temp name: two concurrent CLI invocations both
/// saving used to race on a single `state.json.tmp`, and the loser's rename
/// failed with ENOENT *after* the winner had consumed the file (v0.5.0 makes
/// concurrent invocations normal — the daemon can be auto-started by several
/// processes at once).
fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
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
    let path = super::super::config::paths::state_snapshot_path();
    let bytes = serde_json::to_vec_pretty(snapshot)?;
    write_atomic(&path, &bytes)
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
             run `xscore state migrate` to convert a v2 file, or \
             delete ~/.config/xscore/state/state.json to reset"
        )));
    }
    let snapshot: StateSnapshot = serde_json::from_slice(bytes)
        .map_err(|e| Error::Config(format!("parse {}: {e}", path.display())))?;
    Ok(Some(snapshot))
}

/// v0.6.0 (ADR 0010 D8): convert the on-disk v2 file to v3 in place.
/// Returns the number of sessions created.
///
/// v2 files are never converted implicitly — `parse_snapshot` keeps rejecting
/// them — so this is an explicit, one-shot command. The rules are mechanical:
///
/// * one session per link, named after the link (tmux links become
///   `SessionBackend::Tmux`, raw links `Native`);
/// * every link/window/pane gets the `session_id` of the session its link
///   produced;
/// * children with no link fall back to a `Native` session named `default`,
///   which is created on demand — a pane that was never attached still has to
///   belong somewhere.
///
/// The file is read as raw JSON rather than as v2 structs on purpose: v2 types
/// no longer exist in this crate, and the migration must keep working on files
/// written years ago.
pub fn migrate_v2_to_v3() -> Result<usize> {
    let path = super::super::config::paths::state_snapshot_path();
    migrate_from(&path)
}

fn migrate_from(path: &Path) -> Result<usize> {
    if !path.exists() {
        return Err(Error::Config(format!(
            "no state file at {}; nothing to migrate",
            path.display()
        )));
    }
    let bytes = std::fs::read(path)
        .map_err(|e| Error::Config(format!("read {}: {e}", path.display())))?;
    let root: serde_json::Value = serde_json::from_slice(&bytes)
        .map_err(|e| Error::Config(format!("parse {}: {e}", path.display())))?;
    let found = root.get("schema_version").and_then(|v| v.as_u64()).unwrap_or(0);
    if found != 2 {
        return Err(Error::Config(format!(
            "state.json schema_version={found}; `xscore state migrate` converts a v2 file only"
        )));
    }

    let mut links = json_array(root.get("links"));
    let mut panes = json_array(root.get("panes"));
    let mut windows = json_array(root.get("windows"));

    let mut sessions: Vec<SessionInfo> = Vec::new();
    let mut used_names: HashSet<String> = HashSet::new();
    let mut session_by_link: HashMap<String, SessionId> = HashMap::new();
    let mut default_session: Option<SessionId> = None;

    // --- 1. one session per link ---
    for link in &links {
        let Some(link_id) = link.get("id").and_then(|v| v.as_str()) else {
            continue;
        };
        let base = match link.get("name").and_then(|v| v.as_str()) {
            Some(n) if !n.is_empty() && !used_names.contains(n) => n.to_string(),
            // A duplicate (or empty) link name cannot become a session name:
            // `by_name` must stay unambiguous. Fall back to the id, which is.
            _ => format!("link-{}", id_prefix(link_id)),
        };
        let created_at = link
            .get("created_at")
            .and_then(|v| serde_json::from_value(v.clone()).ok())
            .unwrap_or_else(chrono::Utc::now);

        let is_tmux = link
            .get("sink")
            .and_then(|s| s.get("sink"))
            .and_then(|v| v.as_str())
            == Some("tmux");
        let backend = if is_tmux {
            let session = link
                .get("sink")
                .and_then(|s| s.get("session_name"))
                .and_then(|v| v.as_str())
                .map(str::to_string)
                .unwrap_or_else(|| format!("xscore-{link_id}"));
            SessionBackend::Tmux {
                host: TmuxHostConfig::Local,
                socket: TmuxSocket::Named("xscore".into()),
                session,
            }
        } else {
            SessionBackend::Native
        };

        let info = SessionInfo {
            id: SessionId::new(),
            name: unique_name(&base, &mut used_names),
            created_at,
            backend,
        };
        session_by_link.insert(link_id.to_string(), info.id);
        sessions.push(info);
    }

    // ...then stamp each link with the session it produced. A second pass keeps
    // the ownership rules obvious: the map is built from an immutable view, and
    // the links are edited afterwards.
    for link in &mut links {
        let Some(link_id) = link.get("id").and_then(|v| v.as_str()).map(str::to_string) else {
            continue;
        };
        let Some(sid) = session_by_link.get(&link_id).copied() else {
            continue;
        };
        if let (Some(obj), Ok(id)) = (link.as_object_mut(), serde_json::to_value(sid)) {
            obj.insert("session_id".into(), id);
        }
    }

    // --- 2. panes: their link decides, else the default session ---
    let mut session_by_pane: HashMap<String, SessionId> = HashMap::new();
    for pane in &mut panes {
        let link_id = pane.get("link_id").and_then(|v| v.as_str()).map(str::to_string);
        let sid = match link_id.and_then(|l| session_by_link.get(&l).copied()) {
            Some(sid) => sid,
            None => ensure_default_session(
                &mut sessions, &mut used_names, &mut default_session,
            ),
        };
        if let (Some(obj), Ok(id)) = (
            pane.as_object_mut(),
            serde_json::to_value(sid),
        ) {
            obj.insert("session_id".into(), id);
        }
        if let Some(pid) = pane.get("id").and_then(|v| v.as_str()) {
            session_by_pane.insert(pid.to_string(), sid);
        }
    }

    // --- 3. windows: any pane inside decides, else the default session ---
    for window in &mut windows {
        let sid = window
            .get("panes")
            .and_then(|v| v.as_array())
            .and_then(|ids| {
                ids.iter()
                    .filter_map(|v| v.as_str())
                    .find_map(|pid| session_by_pane.get(pid).copied())
            })
            .unwrap_or_else(|| {
                ensure_default_session(&mut sessions, &mut used_names, &mut default_session)
            });
        if let (Some(obj), Ok(id)) = (window.as_object_mut(), serde_json::to_value(sid)) {
            obj.insert("session_id".into(), id);
        }
    }

    let created = sessions.len();
    let value = serde_json::json!({
        "schema_version": SCHEMA_VERSION,
        "sessions": sessions,
        "links": links,
        "windows": windows,
        "panes": panes,
    });
    // Round-trip through the typed snapshot before writing: it validates the
    // result (a child left with a dangling `session_id` fails here, not on the
    // user's next command) and keeps the on-disk field order identical to
    // `save`'s — `sessions` first. `serde_json::Map` alone would sort the keys.
    let snapshot: StateSnapshot = serde_json::from_value(value).map_err(|e| {
        Error::Config(format!("migrated state does not round-trip: {e}"))
    })?;
    let bytes = serde_json::to_vec_pretty(&snapshot)?;
    write_atomic(path, &bytes)?;
    Ok(created)
}

fn json_array(v: Option<&serde_json::Value>) -> Vec<serde_json::Value> {
    v.and_then(|v| v.as_array()).cloned().unwrap_or_default()
}

/// First 8 characters of a uuid, for a session name derived from an id.
fn id_prefix(id: &str) -> &str {
    id.get(..8).unwrap_or(id)
}

fn unique_name(base: &str, used: &mut HashSet<String>) -> String {
    if used.insert(base.to_string()) {
        return base.to_string();
    }
    let mut n = 2;
    loop {
        let candidate = format!("{base}-{n}");
        if used.insert(candidate.clone()) {
            return candidate;
        }
        n += 1;
    }
}

fn ensure_default_session(
    sessions: &mut Vec<SessionInfo>,
    used_names: &mut HashSet<String>,
    slot: &mut Option<SessionId>,
) -> SessionId {
    if let Some(sid) = slot {
        return *sid;
    }
    let info = SessionInfo::new_native(unique_name("default", used_names));
    let sid = info.id;
    sessions.push(info);
    *slot = Some(sid);
    sid
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn path() -> PathBuf { PathBuf::from("/tmp/does-not-matter.json") }

    fn temp_path(tag: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "xscore-state-{tag}-{}.json",
            std::process::id()
        ))
    }

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
        assert!(msg.contains("expected 3"), "got: {msg}");
        assert!(msg.contains("delete"), "got: {msg}");
        assert!(msg.contains("state migrate"), "got: {msg}");
    }

    #[test]
    fn wrong_schema_version_is_rejected() {
        let bad = br#"{"schema_version":1,"links":[],"windows":[],"panes":[]}"#;
        let err = parse_snapshot(bad, &path()).unwrap_err();
        assert!(format!("{err}").contains("schema_version=1"), "{err}");
    }

    #[test]
    fn v2_file_is_rejected_with_migration_hint() {
        let v2 = br#"{"schema_version":2,"links":[],"windows":[],"panes":[]}"#;
        let err = parse_snapshot(v2, &path()).unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("schema_version=2"), "got: {msg}");
        assert!(msg.contains("expected 3"), "got: {msg}");
        assert!(msg.contains("state migrate"), "got: {msg}");
    }

    #[test]
    fn current_schema_version_roundtrips() {
        let snap = StateSnapshot::default();
        let bytes = serde_json::to_vec(&snap).unwrap();
        let parsed = parse_snapshot(&bytes, &path()).unwrap().unwrap();
        assert_eq!(parsed.schema_version, SCHEMA_VERSION);
        assert!(parsed.sessions.is_empty());
    }

    #[test]
    fn v3_roundtrips_sessions_and_children() {
        let mut snap = StateSnapshot::default();
        let session = SessionInfo::new_native("work");
        let sid = session.id;
        snap.sessions.push(session);
        let bytes = serde_json::to_vec(&snap).unwrap();
        let parsed = parse_snapshot(&bytes, &path()).unwrap().unwrap();
        assert_eq!(parsed.sessions.len(), 1);
        assert_eq!(parsed.sessions[0].id, sid);
        assert_eq!(parsed.sessions[0].name, "work");
    }

    #[test]
    fn sessions_are_written_before_links() {
        // The brief's on-disk order: `sessions` first, so a human reads the
        // workspace before its children.
        let bytes = serde_json::to_vec(&StateSnapshot::default()).unwrap();
        let s = String::from_utf8(bytes).unwrap();
        let sessions_at = s.find("\"sessions\"").unwrap();
        let links_at = s.find("\"links\"").unwrap();
        assert!(sessions_at < links_at, "got: {s}");
    }

    /// v2 fixture: one tmux link, one raw link, one window, two panes (one
    /// bound to the tmux link, one never attached).
    fn v2_fixture() -> String {
        let tmux_link = "01928e0a-7d10-7000-8000-000000000001";
        let raw_link = "01928e0a-7d10-7000-8000-000000000002";
        let window = "01928e0a-7d10-7000-8000-000000000003";
        let tmux_pane = "01928e0a-7d10-7000-8000-000000000004";
        let orphan_pane = "01928e0a-7d10-7000-8000-000000000005";
        format!(
            r#"{{
  "schema_version": 2,
  "links": [
    {{"id": "{tmux_link}", "name": "work", "created_at": "2026-10-01T10:00:00Z",
      "address": {{"transport": "shell", "command": "/bin/bash", "args": []}},
      "transport": {{"transport": "shell", "command": "/bin/bash", "args": []}},
      "sink": {{"sink": "tmux", "mode": "auto", "session_name": "xscore-work"}}}},
    {{"id": "{raw_link}", "name": "local", "created_at": "2026-10-02T10:00:00Z",
      "address": {{"transport": "shell", "command": "/bin/sh", "args": []}},
      "transport": {{"transport": "shell", "command": "/bin/sh", "args": []}},
      "sink": {{"sink": "raw"}}}}
  ],
  "windows": [
    {{"id": "{window}", "name": "w-one", "created_at": "2026-10-01T10:00:00Z",
      "layout": {{"kind": "single"}}, "cols": 120, "rows": 30,
      "tmux_window_handle": "@0", "panes": ["{tmux_pane}", "{orphan_pane}"]}}
  ],
  "panes": [
    {{"id": "{tmux_pane}", "name": "p-one", "created_at": "2026-10-01T10:00:00Z",
      "cols": 120, "rows": 30, "x": 0, "y": 0,
      "link_id": "{tmux_link}", "window_id": "{window}", "sink_handle": "%0"}},
    {{"id": "{orphan_pane}", "name": "p-two", "created_at": "2026-10-01T10:05:00Z",
      "cols": 120, "rows": 30, "x": 0, "y": 0,
      "link_id": null, "window_id": "{window}", "sink_handle": null}}
  ]
}}"#
        )
    }

    #[test]
    fn migrate_v2_creates_sessions_and_repaps_children() {
        let path = temp_path("migrate");
        std::fs::write(&path, v2_fixture()).unwrap();

        let created = migrate_from(&path).unwrap();
        // one per link, plus the `default` session for the unattached pane.
        assert_eq!(created, 3, "sessions created");

        let bytes = std::fs::read(&path).unwrap();
        let snap = parse_snapshot(&bytes, &path).unwrap().unwrap();
        let _ = std::fs::remove_file(&path);

        // Same on-disk order as `save`: schema_version, then sessions.
        let text = String::from_utf8(bytes).unwrap();
        assert!(
            text.find("\"sessions\"").unwrap() < text.find("\"links\"").unwrap(),
            "migrated file should lead with sessions: {text}"
        );

        assert_eq!(snap.schema_version, 3);
        assert_eq!(snap.sessions.len(), 3);

        let ids: HashSet<SessionId> = snap.sessions.iter().map(|s| s.id).collect();
        // every child's session exists
        for link in &snap.links {
            assert!(ids.contains(&link.session_id), "link {link:?} has a dangling session");
        }
        for window in &snap.windows {
            assert!(ids.contains(&window.info.session_id), "window has a dangling session");
        }
        for pane in &snap.panes {
            assert!(ids.contains(&pane.session_id), "pane has a dangling session");
        }

        // names: one per link name + `default`
        let names: HashSet<&str> = snap.sessions.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains("work"), "got {names:?}");
        assert!(names.contains("local"), "got {names:?}");
        assert!(names.contains("default"), "got {names:?}");

        // the tmux link became a tmux session keeping the tmux session name
        let work = snap.sessions.iter().find(|s| s.name == "work").unwrap();
        match &work.backend {
            SessionBackend::Tmux { host, socket, session } => {
                assert_eq!(*host, TmuxHostConfig::Local);
                assert_eq!(*socket, TmuxSocket::Named("xscore".into()));
                assert_eq!(session, "xscore-work");
            }
            other => panic!("expected tmux backend, got {other:?}"),
        }
        let local = snap.sessions.iter().find(|s| s.name == "local").unwrap();
        assert_eq!(local.backend, SessionBackend::Native);

        // links point at their own session; the unattached pane fell back
        let work_sid = work.id;
        let tmux_link = snap.links.iter().find(|l| l.name == "work").unwrap();
        assert_eq!(tmux_link.session_id, work_sid);
        let raw_link = snap.links.iter().find(|l| l.name == "local").unwrap();
        assert_eq!(raw_link.session_id, local.id);

        let bound = snap.panes.iter().find(|p| p.name == "p-one").unwrap();
        assert_eq!(bound.session_id, work_sid);
        let orphan = snap.panes.iter().find(|p| p.name == "p-two").unwrap();
        assert_ne!(orphan.session_id, work_sid);
        let default = snap.sessions.iter().find(|s| s.name == "default").unwrap();
        assert_eq!(default.backend, SessionBackend::Native);

        // the window follows a pane inside it
        assert_eq!(snap.windows[0].info.session_id, work_sid);
    }

    #[test]
    fn migrate_refuses_a_current_file() {
        let path = temp_path("migrate-current");
        let bytes = serde_json::to_vec(&StateSnapshot::default()).unwrap();
        std::fs::write(&path, bytes).unwrap();
        let err = migrate_from(&path).unwrap_err();
        let _ = std::fs::remove_file(&path);
        assert!(format!("{err}").contains("schema_version=3"), "{err}");
    }
}
