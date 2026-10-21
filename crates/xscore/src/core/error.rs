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

    /// v0.3.0 (ADR 0007 v2): the pane has no link attached. Data ops
    /// (`xscore data {read,write,resize}`) fail with this error for
    /// orphan panes. The pane keeps its buffer content — re-attach it
    /// via `xscore pane attach --pane <pid> --link <lid>` to resume I/O.
    #[error("pane {0} is not attached to any link (orphan — attach via `xscore pane attach --pane {0} --link <lid>`)")]
    NotAttached(PaneId),

    /// v0.3.0 (ADR 0007 v2): the window already contains this pane id.
    /// Returned by `add_pane_to_window` when the pane is already in the
    /// window's pane list (idempotent failure surface — explicit errors
    /// over silent no-ops).
    #[error("pane {pane} is already in window {window}")]
    AlreadyInWindow { window: WindowId, pane: PaneId },

    /// v0.3.0 (ADR 0007 v2): the window does NOT contain this pane id.
    /// Returned by `remove_pane_from_window` when the pane isn't in
    /// the window's pane list.
    #[error("pane {pane} is not in window {window}")]
    NotInWindow { window: WindowId, pane: PaneId },

    /// v0.3.0 (ADR 0007 v2): the link does not contain this pane as
    /// active (for `detach_pane` when pane.link_id != lid).
    #[error("pane {pane} is not attached to link {link}")]
    PaneNotOnLink { link: LinkId, pane: PaneId },

    /// v0.4.1 (P0-3): the pane is bound to a link whose transport is no
    /// longer alive (the process that owned it exited). The binding is
    /// real — persisted state is not lying — but no I/O can happen until
    /// the link is re-established. Distinct from `NotAttached` (no
    /// binding at all) on purpose: one is "connection lost", the other
    /// is "never connected".
    ///
    /// v0.5.0 (ADR 0009): with the resident daemon this now means "the
    /// daemon is not holding this link" — i.e. it was stopped or
    /// restarted since the link was created. Re-adding the link (or
    /// `pane attach`) re-creates the transport and clears this.
    #[error("link {0} is not connected (its transport died with the previous process); re-add the link to reconnect")]
    LinkNotConnected(LinkId),

    /// v0.5.0 (ADR 0009): the resident daemon could not be reached, could
    /// not be started, or refused a request. Carries the daemon's own
    /// message so a failure inside the daemon (a transport that won't
    /// open, a pane that is not attached) reads the same as it did when
    /// the work happened in-process.
    #[error("daemon error: {0}")]
    Daemon(String),


    #[error("`xscore link open` is only valid for tmux sessions, not {0}")]
    OpenOnlyForTmux(&'static str),

    #[error("`xscore link close` is only valid for tmux sessions, not {0}")]
    CloseOnlyForTmux(&'static str),

    #[error("link {0} does not use the tmux sink; add `?tmux=on` to the URL to enable window/pane commands")]
    NotTmuxLink(LinkId),

    /// v0.4.0 (ADR 0008): a tmux-backed link's panes are created by
    /// tmux itself (the session's initial window/pane, or a new split
    /// inside a tmux window). Binding a pre-existing hollow pane with
    /// `link add --pane` makes no sense for them.
    #[error("tmux-backed links create panes via tmux; `--pane` is not allowed")]
    TmuxLinkCannotBindPane,

    /// v0.4.0 (ADR 0008): the pane is already attached to another link.
    /// We never steal it silently — the user must detach first.
    #[error("pane {0} is already attached to link {1}; detach it first")]
    PaneAlreadyAttached(PaneId, LinkId),

    /// v0.4.0 (ADR 0008): a tmux-owned pane cannot be detached — it is
    /// an xscore view of a real tmux pane. Delete it with `pane del`.
    #[error("pane {0} is owned by tmux and cannot be detached; use `pane del` instead")]
    TmuxPaneNotDetachable(PaneId),

    #[error("link id already in use: {0}")]
    LinkIdExists(LinkId),

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

    /// ADR 0010 / arch.md §4: `resize` is only real where something owns a
    /// window size — a pty (`TIOCSWINSZ`), an ssh channel (`window-change`) or
    /// a tmux pane (`resize-pane`). A serial link has none, and a hydrated
    /// `NoopTransport` has no connection at all. Returning `Ok(())` for those
    /// was a silent lie: the caller saw success and only `PaneInfo` changed.
    #[error("link kind `{0}` has no window size; resize only applies to a pty, an ssh channel or a tmux pane")]
    ResizeUnsupported(&'static str),
}

pub type Result<T> = std::result::Result<T, Error>;