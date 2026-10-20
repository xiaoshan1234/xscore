// core/pane.rs (v0.4.0 — buffer-per-pane, ADR 0007 v2 / ADR 0008)
//
// `Pane` as an in-memory struct does not exist. The buffer (live bridge
// + ring buffer) and the UI info are stored separately:
//   - `PaneInfo` lives in `PaneRegistry` (JSON-serializable) and carries
//     `sink_handle` (`%N`) when tmux owns the pane (v0.4.0).
//   - The bridge lives in `SessionManager::pane_bridges` and the direct
//     pane's history in `SessionManager::pane_buffers` (both in-memory
//     only; tmux bridges are rebuilt from `sink_handle` on hydration).
//
// This module now only re-exports `PaneInfo` semantics from `model::PaneInfo`.
// (`Pane` is intentionally NOT re-exported here. If a downstream caller needs
// the old aggregate, they should use `crate::core::pane_registry::PaneRegistry`
// + `crate::core::manager::SessionManager::pane_bridges`.)