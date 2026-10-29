//! tmux 目标模型：宿主机与 socket 选择。

#[derive(Debug, Clone, Default, PartialEq)]
pub enum TmuxHostConfig {
    #[default]
    Local,
    Ssh {
        destination: String,
        args: Vec<String>,
    },
    Wsl {
        args: Vec<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TmuxSocket {
    Default,
    Named(String),
    Path(String),
}
