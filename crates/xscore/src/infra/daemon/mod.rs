// daemon/mod.rs (v0.5.0 — resident daemon, ADR 0009)
//
// A direct link's transport (shell child, serial port, ssh session) used
// to live in the CLI process that created it, so it died the moment that
// command exited — which made `link add` followed by `data write` in a
// later invocation impossible by construction. This module is the fix: a
// small resident process that owns
//
//   * every live direct-link transport, and
//   * every pane's byte buffer,
//
// while the CLI keeps owning the metadata in `state.json`.
//
//     daemon/protocol.rs   wire format (length-prefixed JSON + hex)
//     daemon/server.rs     `xscore daemon serve`: the state + pumps
//     daemon/client.rs     auto-start, requests, `DaemonTransport`
//
// tmux-backed links do NOT go through the daemon: tmux already is a
// resident server, so `TmuxBridge` keeps driving `tmux` commands directly.
pub mod client;
pub mod ipc;
pub mod protocol;
pub mod server;

pub use client::{
    ensure_daemon, probe_buffer, probe_link, send, socket_path, DaemonClient, DaemonTransport,
};
pub use protocol::{Request, Response, Status};
pub use server::run;
