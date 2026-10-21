// backends/mod.rs (v0.5.0 — bridge model + ring buffer, ADR 0008/0009)
//
//   - `PaneBridge` has three impls: `DirectBridge` (in-process: wraps the
//     link's `Arc<dyn Transport>` AND a per-pane `RingBuffer`),
//     `DaemonBridge` (v0.5.0: same contract, but the transport and the
//     buffer live in the resident daemon) and `TmuxBridge` (wraps
//     `Weak<TmuxSink>` + the `%N` handle). All in `bridge.rs`.
//   - `RingBuffer` is the pane's buffer core (v0.4.0): a direct pane's
//     history lives here, not in the transport, so it survives detach.
//
// The `BackendRegistry` hashmap is not needed — the bridge lives in the
// manager's per-pane map.

pub mod bridge;
pub mod ring_buffer;

pub use bridge::{ControlBridge, DaemonBridge, DirectBridge, PaneBridge, TmuxBridge};
pub use ring_buffer::RingBuffer;
