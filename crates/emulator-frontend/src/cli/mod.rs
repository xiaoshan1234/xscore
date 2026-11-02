//! CLI 命令面：clap 定义、参数解析、密码边缘处理。
//!
//! 本模块是「用户如何下命令」的壳，不持有任何 UI 状态。GUI 前端不会复用
//! 这里，但会复用其下的 `ui` 应用核心。

mod cmd;
mod parse;
mod secrets;

pub use cmd::{Cmd, DaemonCmd, LsCmd, PaneCmd, TmlAddArgs, TmlCmd, TmxAttachArgs, TmxCmd};
pub use parse::{
    build_tmx_config, parse_args, parse_conversation_url, parse_ssh_target, print_help,
    ConversationSpec,
};
pub use secrets::resolve_secrets;
