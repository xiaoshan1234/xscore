use thiserror::Error;
use super::id::{LinkId, PaneId, WindowId};

#[derive(Debug, Error)]
pub enum Error {
    #[error("link not found: {0}")]
    LinkNotFound(LinkId),

    #[error("window not found: {0}")]
    WindowNotFound(WindowId),

    #[error("pane not found: {0}")]
    PaneNotFound(PaneId),

    #[error("`xscore link open` is only valid for tmux sessions, not {0}")]
    OpenOnlyForTmux(&'static str),

    #[error("`xscore link close` is only valid for tmux sessions, not {0}")]
    CloseOnlyForTmux(&'static str),

    #[error("link {0} does not use the tmux sink; add `?tmux=on` to the URL to enable window/pane commands")]
    NotTmuxLink(LinkId),

    #[error("url parse error: {0}")]
    Url(String),

    #[error("transport error: {0}")]
    Transport(String),

    #[error("sink error: {0}")]
    Sink(String),

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("config error: {0}")]
    Config(String),

    #[error("invalid input: {0}")]
    Invalid(String),

    #[error("serialization error: {0}")]
    Serde(#[from] serde_json::Error),
}

pub type Result<T> = std::result::Result<T, Error>;
