// backends/mod.rs (v0.3.0 — bridge model, ADR 0007 v2)
//
// The legacy `PaneBackend` trait + `BackendRegistry` + `CompositeBackend`
// are GONE in v0.3.0. The new model:
//
//   - Each pane has `Pane.bridge: Mutex<Option<Arc<dyn PaneBridge>>>`.
//   - `PaneBridge` has two impls: `DirectBridge` (wraps the link's
//     `Arc<dyn Transport>` for raw links) and `TmuxBridge` (wraps
//     `Weak<TmuxSink>` for tmux-backed links). Both are in `bridge.rs`.
//   - Data ops (`xscore data {read,write,resize}`) call
//     `pane.bridge.read/write/resize` directly through the manager.
//
// The `BackendRegistry` hashmap is no longer needed — the bridge lives
// on the pane itself.

pub mod bridge;

pub use bridge::{DirectBridge, PaneBridge, TmuxBridge};