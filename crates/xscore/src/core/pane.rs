// core/pane.rs (v0.3.0 — buffer-per-pane, ADR 0007 v2)
//
// v0.3.0: `Pane` as an in-memory struct has been removed. The buffer
// (live bridge) and the UI info are now stored separately:
//   - `PaneInfo` lives in `PaneRegistry` (JSON-serializable).
//   - The bridge lives in `SessionManager::pane_bridges` (in-memory
//     only; not persisted — bridges are rebuilt on `attach_pane`).
//
// This module now only re-exports `PaneInfo` semantics from `model::PaneInfo`.
// The `Pane` struct that used to live here was a v0.2.0 design — see ADR 0007
// v2 for the v0.3.0 split.
//
// (`Pane` is intentionally NOT re-exported here. If a downstream caller needs
// the old aggregate, they should use `crate::core::pane_registry::PaneRegistry`
// + `crate::core::manager::SessionManager::pane_bridges`.)