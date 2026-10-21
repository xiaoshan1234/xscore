// backends/mod.rs (v0.6.0 — the per-pane conduit)
//
//   - `PaneBridge` has three impls: `DirectBridge` (in-process: wraps the
//     link's `Arc<dyn Transport>` AND a per-pane `RingBuffer`),
//     `DaemonBridge` (v0.5.0: same contract, but the transport and the
//     buffer live in the resident daemon) and `ControlBridge` (a pane on its
//     session's tmux control client). All in `bridge.rs`.
//   - The pane's buffer is `core::buffer::RingBuffer`: it belongs to the pane,
//     not to the connection, which is why it is not here (arch.md §9).
//
// The `BackendRegistry` hashmap is not needed — the bridge lives in the
// manager's per-pane map.

pub mod bridge;

pub use bridge::{ControlBridge, DaemonBridge, DirectBridge, PaneBridge};
