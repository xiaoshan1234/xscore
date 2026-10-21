// core/pane.rs (v0.6.0 — buffer-per-pane, ADR 0007 v2 / ADR 0010)
//
// `Pane` as an in-memory struct does not exist. The buffer (live bridge
// + ring buffer) and the UI info are stored separately:
//   - `PaneInfo` lives in `PaneRegistry` (JSON-serializable) and carries
//     `link_key` (`%N`) when tmux owns the pane (ADR 0010 §D6).
//   - The bridge lives in `SessionManager::pane_bridges` and the pane's
//     history in `SessionManager::pane_buffers` (both in-memory only; both
//     are rebuilt on demand, see `SessionManager::bridge_for`).
//
// This module now only re-exports `PaneInfo` semantics from `model::PaneInfo`.
// (`Pane` is intentionally NOT re-exported here. If a downstream caller needs
// the old aggregate, they should use `crate::core::pane_registry::PaneRegistry`
// + `crate::core::manager::SessionManager::pane_bridges`.)