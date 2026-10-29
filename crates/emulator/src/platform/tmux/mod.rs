//! L0 tmux 控制模式客户端：进程、协议解析与文本格式。

pub mod control;
pub mod format;
pub mod model;
pub mod protocol;

pub use control::{argv, TmuxControl};
pub use format::{
    host_prefix, list_panes_command, list_windows_command, parse_pane_line, parse_window_line,
    quote_arg, resize_pane_command, send_keys_literal, socket_args, PaneLine, WindowLine,
    LIST_PANES_FORMAT, LIST_WINDOWS_FORMAT,
};
pub use model::{TmuxHostConfig, TmuxSocket};
pub use protocol::{Parsed, ProtocolParser, TmuxEvent};
