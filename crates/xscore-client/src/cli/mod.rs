//! CLI 命令面：clap 定义、参数解析、密码边缘处理。
//!
//! 本模块只负责把 argv 转换成可发送的协议命令，不持有任何 daemon 状态。

mod cmd;
mod parse;
mod secrets;

pub use cmd::{
    Cmd, DaemonCmd, LogFormatArg, LsCmd, TmlAddArgs, TmlCmd, TmlLogArgs, TmxAttachArgs, TmxCmd,
};
pub use parse::{parse_args, print_help};
pub use secrets::resolve_secrets;
