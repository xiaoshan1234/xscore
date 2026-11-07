//! 命令输入管线：clap 解析、命令模型与 URL 解析、命令映射。

mod args;
mod map;
mod model;

pub use args::{
    parse_args, Cmd, LogFormatArg, LsCmd, TmlAddArgs, TmlCmd, TmlLogArgs, TmxAttachArgs, TmxCmd,
};
pub use map::command_request;
pub use model::{
    parse_conversation_url, parse_environment_assignment, parse_ssh_target, CommandRequest,
    ConversationSpec, LogFormat, LsCommand, TmlAddRequest, TmlCommand, TmlLogRequest,
    TmxAttachRequest, TmxCommand,
};
