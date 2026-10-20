// core/link.rs (v0.3.0 — ADR 0007 v2, decoupled model)
//
// v0.3.0 changes:
//   - `windows: Mutex<Vec<WindowId>>` is GONE. Link no longer owns windows —
//     windows are independent entities tracked by `WindowRegistry`.
//   - `active_pane: Mutex<Option<PaneId>>` is NEW. For direct (raw) links,
//     this tracks the single active pane (Q1 — user-confirmed 2026-10-19):
//     when a new pane is attached via `pane attach`, the previously-active
//     pane is auto-detached (its `link_id` becomes None, bridge is closed).
//     For tmux-backed links this field is not consulted (multiple panes can
//     be attached simultaneously).
//   - `transport: Box<dyn Transport>` → `Arc<dyn Transport>`. The Arc lets
//     us hand the transport to `DirectBridge::transport` (which needs to
//     outlive the link for the lifetime of the bridge) while the link
//     itself still owns the strong ref.
//   - `tmux_sink: Option<Arc<TmuxSink>>` is NEW. Only populated for
//     tmux-backed links. We hold an `Arc<TmuxSink>` (not just `Box<dyn Sink>`)
//     so the `TmuxBridge` can take a `Weak<TmuxSink>` reference — when the
//     link is deleted, the bridge's upgrade() returns None and the bridge
//     returns `Error::Sink("tmux sink dropped")` for any subsequent op.
use std::sync::Arc;
use tokio::sync::Mutex;
use super::id::{LinkId, PaneId};
use crate::model::LinkInfo;
use crate::sink::Sink;
use crate::sink::tmux::TmuxSink;
use crate::transport::Transport;

pub struct Link {
    pub id: LinkId,
    pub info: LinkInfo,
    pub transport: Arc<dyn Transport>,
    pub sink: Box<dyn Sink>,
    /// Concrete `Arc<TmuxSink>` for tmux-backed links (None for raw
    /// links). Used by the manager to build `TmuxBridge` (which takes
    /// `Weak<TmuxSink>` — when the link is dropped, the Weak fails to
    /// upgrade and the bridge surfaces `Error::Sink("tmux sink dropped")`).
    pub tmux_sink: Option<Arc<TmuxSink>>,
    /// Q1 (v0.3.0): current active pane for direct links. `None` means
    /// no pane is attached. For tmux-backed links this field is ignored
    /// (multiple panes can be attached). See ADR 0007 v2 § Q1.
    pub active_pane: Mutex<Option<PaneId>>,
    /// v0.4.1 (P0-3): whether the underlying connection is ALIVE.
    ///
    /// `false` only after hydration: a direct (shell/serial/ssh)
    /// transport is owned by the process that created it, so when that
    /// process exits the connection dies but the persisted binding
    /// (`PaneInfo.link_id`) survives. Keeping this flag separate from
    /// the binding lets `pane show` keep reporting the truth ("this
    /// pane is bound to link X") while data ops report the real
    /// problem ("link X is not connected") instead of the misleading
    /// `NotAttached`. tmux-backed links stay `true` after hydration —
    /// the tmux session outlives the CLI process.
    pub connected: bool,
}

pub type ArcLink = Arc<Link>;