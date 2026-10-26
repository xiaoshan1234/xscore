// service/persistence/mod.rs (v0.6.0 — schema_version = 3, ADR 0010 D8)
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

pub mod migrate;
pub mod snapshot;
pub mod store;

pub use migrate::{migrate_v2_to_v3, migrate_v3_tmux_links};
pub use snapshot::{StateSnapshot, WindowSnapshot, SCHEMA_VERSION};
pub use store::{load, on_disk_version, parse_snapshot, save};

#[cfg(test)]
mod tests {
    use super::migrate::*;
    use super::*;
    use crate::infra::link::TransportConfig;
    use crate::infra::tmux::{TmuxHostConfig, TmuxSocket};
    use crate::service::link_service::EndpointRef;
    use crate::service::session_service::{SessionBackend, SessionId, SessionInfo};
    use crate::ui::WindowBackend;
    use std::collections::HashSet;
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
            crate::service::buffer_service::BufferOrigin::Tmux { pane_handle, .. } if pane_handle == "%2"
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
