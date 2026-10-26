// infra/tmux/ — the control-mode client for a managed session (ADR 0010 §D2).
//
// This is the protocol layer only: it spawns `tmux -CC attach`, parses what
// comes back and exposes commands and events, plus the text formats and
// command lines tmux itself parses. Nothing here decides what a session or a
// pane *is*; `service/session_service/service.rs` owns the wiring (one control client per
// tmux-backed session, panes demultiplexed by `%N`).
//
//   protocol.rs — pure parsing of the control protocol, no IO. Fixed to a
//                 captured tmux 3.5a transcript.
//   control.rs  — the process: argv, reader thread, writer thread, correlation.
//   format.rs   — tmux's text interfaces: `-F` output parsers and the
//                 `send-keys` command line (both measured, see that file).
//
// One client per session, not per pane: it is the session's single link, and
// `%output %N` is addressed, not raced over (ADR 0010 §D3).

pub mod control;
pub mod format;
pub mod model;
pub mod protocol;

pub use control::{argv, socket_args, TmuxControl};
pub use format::{
    host_label, host_prefix, kill_pane_command, list_panes_command, list_windows_command,
    new_window_command, one_shot_argv, parse_pane_line, parse_window_line, quote_arg,
    resize_pane_command, send_keys_literal, split_window_command, PaneLine, WindowLine,
    LIST_PANES_FORMAT, LIST_WINDOWS_FORMAT,
};
pub use model::{TmuxHostConfig, TmuxSocket};
pub use protocol::{Parsed, ProtocolParser, TmuxEvent};
