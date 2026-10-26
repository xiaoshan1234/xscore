// service/link_manager/live.rs (v0.6.0 — one connection inside a session, ADR 0010 §4)
//
// A `Link` is the runtime half of `LinkInfo`: the persisted shape plus the
// live object the bytes actually move through.
//
//   * `Pipe(t)`  — a transport xscore owns (shell / serial / ssh, or the
//                  daemon's handle to one). At most one pane reads it.
//   * `Control`  — the session's tmux control client. The `transport` is a
//                  placeholder (`NoopTransport`: the argv is recorded in
//                  `info.address` for `link show`, but nothing moves bytes
//                  through it), and the live client is `control`, filled in
//                  when the session attaches and cleared when it closes.
//
// `transport` is an `Arc` so a `DirectBridge` can hold it for the lifetime of
// its pump while the link keeps its own strong reference.
//
// There is no "active pane" ledger and no per-link sink object: exclusivity is
// a property of the endpoint kind (a `Pipe` link scan-detaches its previous
// pane on attach, a `Control` link addresses panes by `link_key`), and the tmux
// demultiplexer is `TmuxControl`'s reader loop rather than a trait object.
use std::sync::Arc;

use crate::domain::model::LinkInfo;
use crate::domain::LinkId;
use crate::infra::link::Transport;
use crate::infra::tmux::TmuxControl;

pub struct Link {
    pub id: LinkId,
    pub info: LinkInfo,
    pub transport: Arc<dyn Transport>,
    /// The session's control client, once attached. `None` for a `Pipe` link
    /// and for a `Control` link that nothing has attached yet (a fresh CLI
    /// process hydrates the row before it connects).
    pub control: Option<Arc<TmuxControl>>,
    /// v0.4.1 (P0-3): whether the underlying connection is ALIVE.
    ///
    /// `false` only after hydration: a pipe transport is owned by the process
    /// that created it, so when that process exits the connection dies but the
    /// persisted binding (`PaneInfo.link_id`) survives. Keeping this flag
    /// separate from the binding lets `pane show` keep reporting the truth
    /// ("this pane is bound to link X") while data ops report the real problem
    /// ("link X is not connected") instead of the misleading `NotAttached`.
    ///
    /// A `Control` link is not governed by this flag: its liveness is the
    /// session's, so `link_is_connected` asks the control client (or the
    /// server) instead.
    pub connected: bool,
}

pub type ArcLink = Arc<Link>;
