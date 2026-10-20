// backends/mod.rs (v0.4.0 — bridge model + ring buffer, ADR 0008)
//
//   - `PaneBridge` has two impls: `DirectBridge` (wraps the link's
//     `Arc<dyn Transport>` AND a per-pane `RingBuffer`) and `TmuxBridge`
//     (wraps `Weak<TmuxSink>` + the `%N` handle). Both are in `bridge.rs`.
//   - `RingBuffer` is the pane's buffer core (v0.4.0): a direct pane's
//     history lives here, not in the transport, so it survives detach.
//
// The `BackendRegistry` hashmap is not needed — the bridge lives in the
// manager's per-pane map.

pub mod bridge;
pub mod ring_buffer;

pub use bridge::{DirectBridge, PaneBridge, TmuxBridge};
pub use ring_buffer::RingBuffer;
