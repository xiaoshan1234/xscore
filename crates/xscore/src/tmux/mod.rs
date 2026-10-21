// tmux/ — the control-mode client for a managed session (ADR 0010 §D2).
//
// This is the protocol layer only: it spawns `tmux -C attach`, parses what
// comes back and exposes commands and events. Nothing here decides what a
// session or a pane *is*; wiring the client into the daemon, `core/manager.rs`
// and the Sink demultiplexer is a separate change (ADR 0010 §D4), so the
// existing tmux Sink path is untouched by this module.
//
//   protocol.rs — pure parsing, no IO. Fixed to a captured tmux 3.5a transcript.
//   control.rs  — the process: argv, reader thread, writer thread, correlation.
//
// One client per session, not per pane: it is the session's single link, and
// `%output %N` is addressed, not raced over (ADR 0010 §D3).

pub mod control;
pub mod protocol;

pub use control::{argv, socket_args, TmuxControl};
pub use protocol::{Parsed, ProtocolParser, TmuxEvent};
