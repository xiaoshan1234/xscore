use super::snapshot::*;
use super::store::*;
use crate::infra::tmux::{TmuxHostConfig, TmuxSocket};
use crate::service::buffer_service::{BufferId, BufferInfo, BufferOrigin};
use crate::service::link_service::LinkId;
use crate::service::session_service::SessionId;
use crate::service::session_service::{SessionBackend, SessionInfo};
use crate::{Error, Result};
use std::collections::{HashMap, HashSet};
use std::path::Path;

pub fn migrate_v2_to_v3() -> Result<usize> {
    let path = crate::infra::config::paths::state_snapshot_path();
    migrate_v2_from(&path, &crate::infra::util::socket_name())
}

pub(super) fn migrate_v2_from(path: &Path, socket: &str) -> Result<usize> {
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
    let path = crate::infra::config::paths::state_snapshot_path();
    migrate_v3_from(&path, &crate::infra::util::socket_name())
}

pub(super) fn migrate_v3_from(path: &Path, socket: &str) -> Result<usize> {
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
