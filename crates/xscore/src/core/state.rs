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

use crate::core::{BufferId, LinkId, PaneId, SessionId};
use crate::core::{Error, Result};
use crate::model::{
    BufferInfo, BufferOrigin, LinkInfo, PaneInfo, SessionBackend, SessionInfo, TmuxHostConfig,
    TmuxSocket, WindowInfo,
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
    #[serde(default)]
    pub buffers: Vec<BufferInfo>,
    pub windows: Vec<WindowSnapshot>,
    pub panes: Vec<PaneInfo>,
}

impl Default for StateSnapshot {
    fn default() -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            sessions: Vec::new(),
            links: Vec::new(),
            buffers: Vec::new(),
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
    let path = super::super::config::paths::state_snapshot_path();
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
/// The rows of a tmux link are then dropped exactly as `migrate_v3_tmux_links`
/// drops them (see `adopt_rows`): that link was the per-command tmux path, its
/// window/pane rows describe a mechanism this build no longer has, and the
/// session re-enumerates itself on its next attach.
///
/// The file is read as raw JSON rather than as v2 structs on purpose: v2 types
/// no longer exist in this crate, and the migration must keep working on files
/// written years ago.
pub fn migrate_v2_to_v3() -> Result<usize> {
    let path = super::super::config::paths::state_snapshot_path();
    migrate_v2_from(&path, &crate::util::socket_name())
}

fn migrate_v2_from(path: &Path, socket: &str) -> Result<usize> {
    let root = read_json(path, "a v2 file")?;
    let found = root
        .get("schema_version")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    if found != 2 {
        return Err(Error::Config(format!(
            "state.json schema_version={found}; `state migrate` converts a v2 file only"
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
            SessionBackend::Tmux {
                host: TmuxHostConfig::Local,
                socket: TmuxSocket::Named(socket.to_string()),
                session: tmux_session_name(link),
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
        let link_id = pane
            .get("link_id")
            .and_then(|v| v.as_str())
            .map(str::to_string);
        let sid = match link_id.and_then(|l| session_by_link.get(&l).copied()) {
            Some(sid) => sid,
            None => ensure_default_session(&mut sessions, &mut used_names, &mut default_session),
        };
        if let (Some(obj), Ok(id)) = (pane.as_object_mut(), serde_json::to_value(sid)) {
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
    let mut sessions = sessions
        .into_iter()
        .map(|s| serde_json::to_value(s).unwrap_or(serde_json::Value::Null))
        .collect::<Vec<_>>();
    // Same tail as the v3 upgrade: the old tmux links and their rows go, and
    // every surviving row is rewritten into the shape this build writes.
    adopt_rows(&mut sessions, &mut links, &mut windows, &mut panes, socket)?;
    let mut buffers = Vec::new();
    materialize_buffers(&mut panes, &mut buffers)?;
    write_snapshot(path, sessions, links, buffers, windows, panes)?;
    Ok(created)
}

/// v0.6.0 PR4: upgrade a v3 file written before the control client became the
/// only tmux path. Returns the number of sessions converted.
///
/// v3 was never released, so the version stays 3 — but a v3 file from the old
/// build must not silently lose the user's tmux session, which is what a plain
/// re-serialize would do. The rules:
///
/// * a link carrying `sink: { sink: "tmux", .. }` marks its session as
///   `SessionBackend::Tmux` (host `Local`, socket `Named(<XSCORE_SOCKET or
///   xscore>)` — the socket the old code resolved, session name from the sink or
///   `xscore-<link id>`);
/// * **that session's link, window and pane rows are dropped**: the link was the
///   per-command tmux path, the rows described its structure, and the session
///   re-enumerates itself over the control client on its next attach;
/// * native sessions and their rows are untouched (they are only rewritten in
///   the new field shape);
/// * every surviving row is rewritten into the shape this build writes, so the
///   migrated file loads without another pass.
///
/// Running it on an already-migrated file is a no-op (0 converted).
pub fn migrate_v3_tmux_links() -> Result<usize> {
    let path = super::super::config::paths::state_snapshot_path();
    migrate_v3_from(&path, &crate::util::socket_name())
}

fn migrate_v3_from(path: &Path, socket: &str) -> Result<usize> {
    let root = read_json(path, "a v3 file")?;
    let found = root
        .get("schema_version")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    if found != SCHEMA_VERSION as u64 {
        return Err(Error::Config(format!(
            "state.json schema_version={found}; `state migrate` upgrades a v3 file here — \
             run it once for a v2 file first"
        )));
    }

    let mut sessions = json_array(root.get("sessions"));
    let mut links = json_array(root.get("links"));
    let mut buffers = json_array(root.get("buffers"));
    let mut windows = json_array(root.get("windows"));
    let mut panes = json_array(root.get("panes"));

    let converted = adopt_rows(&mut sessions, &mut links, &mut windows, &mut panes, socket)?;
    materialize_buffers(&mut panes, &mut buffers)?;
    write_snapshot(path, sessions, links, buffers, windows, panes)?;
    Ok(converted)
}

/// The shared tail of both migrations: turn old tmux links into tmux sessions,
/// drop the rows that described them, and rewrite the survivors.
fn adopt_rows(
    sessions: &mut [serde_json::Value],
    links: &mut Vec<serde_json::Value>,
    windows: &mut Vec<serde_json::Value>,
    panes: &mut Vec<serde_json::Value>,
    socket: &str,
) -> Result<usize> {
    // --- 1. which sessions were driven by the old per-command tmux path? ---
    let mut converted: HashSet<String> = HashSet::new();
    let mut converted_count = 0usize;
    for link in links.iter() {
        let is_tmux = link
            .get("sink")
            .and_then(|s| s.get("sink"))
            .and_then(|v| v.as_str())
            == Some("tmux");
        if !is_tmux {
            continue;
        }
        let Some(sid) = link
            .get("session_id")
            .and_then(|v| v.as_str())
            .map(str::to_string)
        else {
            continue;
        };
        if !converted.insert(sid.clone()) {
            continue;
        }
        let backend = serde_json::json!({
            "kind": "tmux",
            "host": { "kind": "local" },
            "socket": { "kind": "named", "name": socket },
            "session": tmux_session_name(link),
        });
        if let Some(session) = sessions
            .iter_mut()
            .find(|s| s.get("id").and_then(|v| v.as_str()) == Some(sid.as_str()))
        {
            if let Some(obj) = session.as_object_mut() {
                obj.insert("backend".into(), backend);
                converted_count += 1;
            }
        }
    }

    // --- 2. drop those rows: they belong to the mechanism being removed -----
    if !converted.is_empty() {
        let keep = |row: &serde_json::Value| -> bool {
            row.get("session_id")
                .and_then(|v| v.as_str())
                .map(|sid| !converted.contains(sid))
                .unwrap_or(true)
        };
        links.retain(keep);
        windows.retain(keep);
        panes.retain(keep);

        // A surviving pane must not point at a row that was just dropped: a
        // dangling `window_id` would show up in `pane show` as a window that
        // does not exist.
        let live_windows: HashSet<String> = windows
            .iter()
            .filter_map(|w| w.get("id").and_then(|v| v.as_str()).map(str::to_string))
            .collect();
        let live_links: HashSet<String> = links
            .iter()
            .filter_map(|l| l.get("id").and_then(|v| v.as_str()).map(str::to_string))
            .collect();
        for pane in panes.iter_mut() {
            let Some(obj) = pane.as_object_mut() else {
                continue;
            };
            let dangling_window = obj
                .get("window_id")
                .and_then(|v| v.as_str())
                .map(|wid| !live_windows.contains(wid))
                .unwrap_or(false);
            if dangling_window {
                obj.insert("window_id".into(), serde_json::Value::Null);
            }
            let dangling_link = obj
                .get("link_id")
                .and_then(|v| v.as_str())
                .map(|lid| !live_links.contains(lid))
                .unwrap_or(false);
            if dangling_link {
                obj.insert("link_id".into(), serde_json::Value::Null);
            }
        }
    }

    // --- 3. rewrite every surviving row into the shape this build writes ----
    for link in links.iter_mut() {
        let Some(obj) = link.as_object_mut() else {
            continue;
        };
        // Already in the shape this build writes (a second run): leave the
        // endpoint alone.
        if obj.get("endpoint").map(|v| v.is_object()).unwrap_or(false) {
            obj.remove("sink");
            obj.remove("transport");
            continue;
        }
        let is_control = obj.get("endpoint").and_then(|v| v.as_str()) == Some("control");
        let endpoint = if is_control {
            serde_json::json!("control")
        } else {
            // A pipe link owns a transport; the old shape stored it beside
            // `sink` instead of inside the endpoint.
            let Some(transport) = obj.get("transport").cloned() else {
                let id = obj.get("id").and_then(|v| v.as_str()).unwrap_or("?");
                return Err(Error::Config(format!(
                    "link {id} has no transport in the old file; cannot convert it to a pipe endpoint"
                )));
            };
            serde_json::json!({ "pipe": transport })
        };
        obj.remove("sink");
        obj.remove("transport");
        obj.insert("endpoint".into(), endpoint);
    }

    let mut panes_seen: Vec<(String, String)> = Vec::new();
    for window in windows.iter_mut() {
        let Some(obj) = window.as_object_mut() else {
            continue;
        };
        if obj.get("backend").is_some() {
            // Already in the shape this build writes (a second run).
            obj.remove("tmux_window_handle");
            obj.remove("layout");
            continue;
        }
        let backend = match obj.get("tmux_window_handle").and_then(|v| v.as_str()) {
            Some(handle) => serde_json::json!({ "kind": "tmux", "handle": handle }),
            None => serde_json::json!({ "kind": "native" }),
        };
        obj.remove("tmux_window_handle");
        obj.remove("layout");
        obj.insert("backend".into(), backend);
    }
    for pane in panes.iter_mut() {
        let Some(obj) = pane.as_object_mut() else {
            continue;
        };
        // A pane's address inside a shared link: old files called it
        // `sink_handle`; only tmux panes ever had one.
        if obj.get("link_key").map(|v| v.is_null()).unwrap_or(true) {
            if let Some(handle) = obj
                .get("sink_handle")
                .and_then(|v| v.as_str())
                .map(str::to_string)
            {
                obj.insert("link_key".into(), serde_json::json!(handle));
                if let (Some(pid), Some(lid)) = (
                    obj.get("id").and_then(|v| v.as_str()),
                    obj.get("link_id").and_then(|v| v.as_str()),
                ) {
                    panes_seen.push((pid.to_string(), lid.to_string()));
                }
            }
        }
        obj.remove("sink_handle");
    }
    // Every converted pane must point at a link that still exists; the v3 rule
    // is that a pane's link is what addresses it, so a dangling one would be a
    // silent lie rather than a recoverable state.
    let live: HashSet<&str> = links
        .iter()
        .filter_map(|l| l.get("id").and_then(|v| v.as_str()))
        .collect();
    for (pid, lid) in &panes_seen {
        if !live.contains(lid.as_str()) {
            return Err(Error::Config(format!(
                "pane {pid} is addressed on link {lid}, which is not in the file; \
                 the state is inconsistent"
            )));
        }
    }

    Ok(converted_count)
}

/// The tmux session name an old tmux link used: `?name=` if it had one, else the
/// generated `xscore-<link id>` (ADR 0010 §D7: that prefix is how the old path
/// was recognised, so it is also how it is translated).
fn tmux_session_name(link: &serde_json::Value) -> String {
    link.get("sink")
        .and_then(|s| s.get("session_name"))
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .unwrap_or_else(|| {
            let id = link.get("id").and_then(|v| v.as_str()).unwrap_or("unknown");
            format!("xscore-{id}")
        })
}

fn read_json(path: &Path, what: &str) -> Result<serde_json::Value> {
    if !path.exists() {
        return Err(Error::Config(format!(
            "no state file at {}; nothing to migrate",
            path.display()
        )));
    }
    let bytes =
        std::fs::read(path).map_err(|e| Error::Config(format!("read {}: {e}", path.display())))?;
    serde_json::from_slice(&bytes)
        .map_err(|e| Error::Config(format!("parse {} as {what}: {e}", path.display())))
}

/// Convert old PaneInfo link fields into independent Buffer rows. Buffer IDs
/// are stable within one migration and reused by every pane showing the same
/// source.
fn materialize_buffers(
    panes: &mut [serde_json::Value],
    buffers: &mut Vec<serde_json::Value>,
) -> Result<()> {
    let mut by_source: HashMap<String, BufferId> = buffers
        .iter()
        .filter_map(|buffer| {
            let id = BufferId::parse(buffer.get("id")?.as_str()?).ok()?;
            let origin = buffer.get("origin")?;
            let link = origin.get("link_id")?.as_str()?;
            let key = match origin.get("kind").and_then(|v| v.as_str()) {
                Some("tmux") => format!("{link}:{}", origin.get("pane_handle")?.as_str()?),
                _ => link.to_string(),
            };
            Some((key, id))
        })
        .collect();

    for pane in panes.iter_mut() {
        let Some(obj) = pane.as_object_mut() else {
            continue;
        };
        if obj.get("buffer_id").and_then(|v| v.as_str()).is_some() {
            obj.remove("link_id");
            obj.remove("link_key");
            obj.remove("sink_handle");
            continue;
        }
        let link_id = obj
            .get("link_id")
            .and_then(|v| v.as_str())
            .map(str::to_string);
        let handle = obj
            .get("link_key")
            .or_else(|| obj.get("sink_handle"))
            .and_then(|v| v.as_str())
            .map(str::to_string);
        let buffer_id = match link_id {
            Some(link_id) => {
                let source = handle
                    .as_ref()
                    .map(|handle| format!("{link_id}:{handle}"))
                    .unwrap_or_else(|| link_id.clone());
                if let Some(id) = by_source.get(&source).copied() {
                    id
                } else {
                    let id = BufferId::new();
                    let session_id = obj
                        .get("session_id")
                        .and_then(|v| v.as_str())
                        .and_then(|s| SessionId::parse(s).ok())
                        .ok_or_else(|| {
                            Error::Config("pane has no valid session_id during migration".into())
                        })?;
                    let origin = match &handle {
                        Some(handle) => BufferOrigin::Tmux {
                            link_id: LinkId::parse(&link_id)
                                .map_err(|e| Error::Config(e.to_string()))?,
                            pane_handle: handle.clone(),
                        },
                        None => BufferOrigin::Link {
                            link_id: LinkId::parse(&link_id)
                                .map_err(|e| Error::Config(e.to_string()))?,
                        },
                    };
                    let info = BufferInfo {
                        id,
                        session_id,
                        name: format!("b-{}", &id.to_string()[..8]),
                        created_at: chrono::Utc::now(),
                        origin,
                    };
                    buffers.push(serde_json::to_value(info)?);
                    by_source.insert(source, id);
                    id
                }
            }
            None => {
                obj.insert("buffer_id".into(), serde_json::Value::Null);
                obj.remove("link_id");
                obj.remove("link_key");
                obj.remove("sink_handle");
                continue;
            }
        };
        obj.insert("buffer_id".into(), serde_json::json!(buffer_id.to_string()));
        obj.remove("link_id");
        obj.remove("link_key");
        obj.remove("sink_handle");
    }
    Ok(())
}

/// Round-trip through the typed snapshot before writing: it validates the
/// result (a child left with a dangling `session_id` fails here, not on the
/// user's next command) and keeps the on-disk field order identical to
/// `save`'s — `sessions` first. `serde_json::Map` alone would sort the keys.
fn write_snapshot(
    path: &Path,
    sessions: Vec<serde_json::Value>,
    links: Vec<serde_json::Value>,
    buffers: Vec<serde_json::Value>,
    windows: Vec<serde_json::Value>,
    panes: Vec<serde_json::Value>,
) -> Result<()> {
    let value = serde_json::json!({
        "schema_version": SCHEMA_VERSION,
        "sessions": sessions,
        "links": links,
        "buffers": buffers,
        "windows": windows,
        "panes": panes,
    });
    let snapshot: StateSnapshot = serde_json::from_value(value)
        .map_err(|e| Error::Config(format!("migrated state does not round-trip: {e}")))?;
    let bytes = serde_json::to_vec_pretty(&snapshot)?;
    write_atomic(path, &bytes)
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
    use crate::model::{EndpointRef, TransportConfig, WindowBackend};
    use std::path::PathBuf;

    fn path() -> PathBuf {
        PathBuf::from("/tmp/does-not-matter.json")
    }

    fn temp_path(tag: &str) -> PathBuf {
        std::env::temp_dir().join(format!("xscore-state-{tag}-{}.json", std::process::id()))
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

    /// An old-shaped v3 file is refused, not silently loaded with the tmux
    /// sessions missing — the version gate cannot see that, the row shapes can.
    #[test]
    fn legacy_v3_rows_are_rejected_with_migration_hint() {
        let legacy = v3_legacy_fixture();
        let err = parse_snapshot(legacy.as_bytes(), &path()).unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("older v3 build"), "got: {msg}");
        assert!(msg.contains("state migrate"), "got: {msg}");
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
    fn migrate_v2_creates_sessions_and_drops_the_old_tmux_rows() {
        let path = temp_path("migrate");
        std::fs::write(&path, v2_fixture()).unwrap();

        let created = migrate_v2_from(&path, "test-sock").unwrap();
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
        for link in &snap.links {
            assert!(
                ids.contains(&link.session_id),
                "link has a dangling session"
            );
        }
        for window in &snap.windows {
            assert!(
                ids.contains(&window.info.session_id),
                "window has a dangling session"
            );
        }
        for pane in &snap.panes {
            assert!(
                ids.contains(&pane.session_id),
                "pane has a dangling session"
            );
        }

        let names: HashSet<&str> = snap.sessions.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains("work"), "got {names:?}");
        assert!(names.contains("local"), "got {names:?}");
        assert!(names.contains("default"), "got {names:?}");

        // The tmux link became a tmux session on the socket the migration
        // resolved, keeping the tmux session name it had.
        let work = snap.sessions.iter().find(|s| s.name == "work").unwrap();
        match &work.backend {
            SessionBackend::Tmux {
                host,
                socket,
                session,
            } => {
                assert_eq!(*host, TmuxHostConfig::Local);
                assert_eq!(*socket, TmuxSocket::Named("test-sock".into()));
                assert_eq!(session, "xscore-work");
            }
            other => panic!("expected tmux backend, got {other:?}"),
        }
        let local = snap.sessions.iter().find(|s| s.name == "local").unwrap();
        assert_eq!(local.backend, SessionBackend::Native);

        // The old per-command tmux link and the rows that described it are
        // gone (the session re-enumerates itself over the control client).
        assert_eq!(
            snap.links.len(),
            1,
            "only the native link survives: {:?}",
            snap.links
        );
        let raw_link = snap.links.iter().find(|l| l.name == "local").unwrap();
        assert_eq!(raw_link.session_id, local.id);
        match &raw_link.endpoint {
            EndpointRef::Pipe(TransportConfig::Shell { command, .. }) => {
                assert_eq!(command, "/bin/sh");
            }
            other => panic!("expected a pipe endpoint, got {other:?}"),
        }
        assert!(
            snap.windows.is_empty(),
            "the tmux window row went with its link: {:?}",
            snap.windows
        );
        assert_eq!(
            snap.panes.len(),
            1,
            "only the unattached pane survives: {:?}",
            snap.panes
        );
        let orphan = &snap.panes[0];
        assert_eq!(orphan.name, "p-two");
        assert_eq!(
            orphan.session_id,
            snap.sessions
                .iter()
                .find(|s| s.name == "default")
                .unwrap()
                .id
        );
        assert!(
            orphan.buffer_id.is_none(),
            "an unattached pane has no buffer"
        );
        assert!(orphan.window_id.is_none(), "its window was dropped too");
    }

    #[test]
    fn migrate_refuses_a_current_file() {
        let path = temp_path("migrate-current");
        let bytes = serde_json::to_vec(&StateSnapshot::default()).unwrap();
        std::fs::write(&path, bytes).unwrap();
        let err = migrate_v2_from(&path, "x").unwrap_err();
        let _ = std::fs::remove_file(&path);
        assert!(format!("{err}").contains("schema_version=3"), "{err}");
    }

    /// A v3 file as the pre-PR4 build wrote it: one session driven by the old
    /// per-command tmux link, one native session, and one managed tmux session
    /// with a control link.
    fn v3_legacy_fixture() -> String {
        let s_native = "01928e0a-7d10-7000-8000-000000000101";
        let s_old = "01928e0a-7d10-7000-8000-000000000102";
        let s_work = "01928e0a-7d10-7000-8000-000000000103";
        let l_native = "01928e0a-7d10-7000-8000-000000000201";
        let l_old = "01928e0a-7d10-7000-8000-000000000202";
        let l_ctl = "01928e0a-7d10-7000-8000-000000000203";
        let w_native = "01928e0a-7d10-7000-8000-000000000301";
        let w_old = "01928e0a-7d10-7000-8000-000000000302";
        let w_work = "01928e0a-7d10-7000-8000-000000000303";
        let p_native = "01928e0a-7d10-7000-8000-000000000401";
        let p_old = "01928e0a-7d10-7000-8000-000000000402";
        let p_orphan = "01928e0a-7d10-7000-8000-000000000403";
        let p_work = "01928e0a-7d10-7000-8000-000000000404";
        format!(
            r#"{{
  "schema_version": 3,
  "sessions": [
    {{"id": "{s_native}", "name": "base", "created_at": "2026-10-01T10:00:00Z",
      "backend": {{"kind": "native"}}}},
    {{"id": "{s_old}", "name": "old", "created_at": "2026-10-01T10:00:00Z",
      "backend": {{"kind": "tmux", "host": {{"kind": "local"}},
                 "socket": {{"kind": "default"}}, "session": "xscore-old"}}}},
    {{"id": "{s_work}", "name": "work", "created_at": "2026-10-01T10:00:00Z",
      "backend": {{"kind": "tmux", "host": {{"kind": "local"}},
                 "socket": {{"kind": "named", "name": "xscore-test"}},
                 "session": "work"}}}}
  ],
  "links": [
    {{"id": "{l_native}", "session_id": "{s_native}", "name": "shell", "created_at": "2026-10-01T10:00:00Z",
      "address": {{"transport": "shell", "command": "/bin/sh", "args": []}},
      "transport": {{"transport": "shell", "command": "/bin/sh", "args": []}},
      "sink": {{"sink": "raw"}}, "endpoint": "pipe"}},
    {{"id": "{l_old}", "session_id": "{s_old}", "name": "old", "created_at": "2026-10-01T10:00:00Z",
      "address": {{"transport": "shell", "command": "/bin/cat", "args": []}},
      "transport": {{"transport": "shell", "command": "/bin/cat", "args": []}},
      "sink": {{"sink": "tmux", "mode": "auto", "session_name": "xscore-build"}},
      "endpoint": "pipe"}},
    {{"id": "{l_ctl}", "session_id": "{s_work}", "name": "work-ctl", "created_at": "2026-10-01T10:00:00Z",
      "address": {{"transport": "shell", "command": "tmux", "args": []}},
      "transport": {{"transport": "shell", "command": "tmux", "args": []}},
      "sink": {{"sink": "raw"}}, "endpoint": "control"}}
  ],
  "windows": [
    {{"id": "{w_native}", "session_id": "{s_native}", "name": "n", "created_at": "2026-10-01T10:00:00Z",
      "layout": {{"kind": "single"}}, "cols": 80, "rows": 24,
      "tmux_window_handle": null, "panes": ["{p_native}"]}},
    {{"id": "{w_old}", "session_id": "{s_old}", "name": "o", "created_at": "2026-10-01T10:00:00Z",
      "layout": {{"kind": "single"}}, "cols": 80, "rows": 24,
      "tmux_window_handle": "@0", "panes": ["{p_old}"]}},
    {{"id": "{w_work}", "session_id": "{s_work}", "name": "w", "created_at": "2026-10-01T10:00:00Z",
      "layout": {{"kind": "single"}}, "cols": 80, "rows": 24,
      "tmux_window_handle": "@2", "panes": ["{p_work}"]}}
  ],
  "panes": [
    {{"id": "{p_native}", "session_id": "{s_native}", "name": "n1", "created_at": "2026-10-01T10:00:00Z",
      "cols": 80, "rows": 24, "x": 0, "y": 0,
      "link_id": "{l_native}", "window_id": "{w_native}", "sink_handle": null, "link_key": null}},
    {{"id": "{p_old}", "session_id": "{s_old}", "name": "o1", "created_at": "2026-10-01T10:00:00Z",
      "cols": 80, "rows": 24, "x": 0, "y": 0,
      "link_id": "{l_old}", "window_id": "{w_old}", "sink_handle": "%0", "link_key": null}},
    {{"id": "{p_orphan}", "session_id": "{s_old}", "name": "o2", "created_at": "2026-10-01T10:00:00Z",
      "cols": 80, "rows": 24, "x": 0, "y": 0,
      "link_id": null, "window_id": "{w_old}", "sink_handle": null, "link_key": null}},
    {{"id": "{p_work}", "session_id": "{s_work}", "name": "w1", "created_at": "2026-10-01T10:00:00Z",
      "cols": 80, "rows": 24, "x": 0, "y": 0,
      "link_id": "{l_ctl}", "window_id": "{w_work}", "sink_handle": null, "link_key": "%2"}}
  ]
}}"#
        )
    }

    #[test]
    fn migrate_v3_converts_the_old_tmux_session_and_drops_its_rows() {
        let path = temp_path("migrate-v3");
        std::fs::write(&path, v3_legacy_fixture()).unwrap();

        let converted = migrate_v3_from(&path, "test-sock").unwrap();
        assert_eq!(converted, 1, "one session used the old tmux path");

        let bytes = std::fs::read(&path).unwrap();
        let snap = parse_snapshot(&bytes, &path).unwrap().unwrap();
        let text = String::from_utf8(bytes).unwrap();
        let _ = std::fs::remove_file(&path);

        // The old tmux session is now driven through the control client, on
        // the socket the migration resolved and under the tmux name it had.
        let old = snap.sessions.iter().find(|s| s.name == "old").unwrap();
        match &old.backend {
            SessionBackend::Tmux {
                host,
                socket,
                session,
            } => {
                assert_eq!(*host, TmuxHostConfig::Local);
                assert_eq!(*socket, TmuxSocket::Named("test-sock".into()));
                assert_eq!(session, "xscore-build", "the sink's session name was kept");
            }
            other => panic!("expected a tmux backend, got {other:?}"),
        }

        // Its link, window and both of its panes are gone: the control link is
        // created when the session is next attached, and tmux re-enumerates.
        assert!(
            !snap.links.iter().any(|l| l.session_id == old.id),
            "{:?}",
            snap.links
        );
        assert!(!snap.windows.iter().any(|w| w.info.session_id == old.id));
        assert!(!snap.panes.iter().any(|p| p.session_id == old.id));

        // The native session is untouched.
        let base = snap.sessions.iter().find(|s| s.name == "base").unwrap();
        let native_link = snap.links.iter().find(|l| l.session_id == base.id).unwrap();
        assert!(matches!(native_link.endpoint, EndpointRef::Pipe(_)));
        let native_window = snap
            .windows
            .iter()
            .find(|w| w.info.session_id == base.id)
            .unwrap();
        assert_eq!(native_window.info.backend, WindowBackend::Native);
        assert_eq!(
            snap.panes
                .iter()
                .filter(|p| p.session_id == base.id)
                .count(),
            1
        );

        // The managed session keeps its control link and its rows; the shape is
        // the one this build writes.
        let work = snap.sessions.iter().find(|s| s.name == "work").unwrap();
        let ctl = snap.links.iter().find(|l| l.session_id == work.id).unwrap();
        assert!(ctl.is_control(), "the control endpoint survived: {ctl:?}");
        let work_window = snap
            .windows
            .iter()
            .find(|w| w.info.session_id == work.id)
            .unwrap();
        assert_eq!(
            work_window.info.backend,
            WindowBackend::Tmux {
                handle: "@2".into()
            }
        );
        let work_pane = snap.panes.iter().find(|p| p.session_id == work.id).unwrap();
        let work_buffer = snap
            .buffers
            .iter()
            .find(|buffer| Some(buffer.id) == work_pane.buffer_id)
            .expect("tmux pane buffer");
        assert!(matches!(
            &work_buffer.origin,
            crate::model::BufferOrigin::Tmux { pane_handle, .. } if pane_handle == "%2"
        ));

        // No row anywhere still carries the deleted path's fields.
        assert!(!text.contains("\"sink\""), "the sink key survived: {text}");
        assert!(!text.contains("sink_handle"));
        assert!(!text.contains("tmux_window_handle"));
        assert!(!text.contains("\"layout\""));
        assert_eq!(text.matches("\"endpoint\"").count(), snap.links.len());
    }

    #[test]
    fn migrate_v3_is_idempotent() {
        let path = temp_path("migrate-v3-twice");
        std::fs::write(&path, v3_legacy_fixture()).unwrap();
        assert_eq!(migrate_v3_from(&path, "test-sock").unwrap(), 1);
        let once = std::fs::read(&path).unwrap();

        assert_eq!(
            migrate_v3_from(&path, "test-sock").unwrap(),
            0,
            "nothing left to convert"
        );
        let twice = std::fs::read(&path).unwrap();
        let _ = std::fs::remove_file(&path);
        assert_eq!(once, twice, "a second run must not rewrite the file");
    }
}
