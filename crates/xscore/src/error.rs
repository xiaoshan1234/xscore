use crate::service::link_service::LinkId;
use crate::service::session_service::SessionId;
use crate::ui::{PaneId, WindowId};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("link not found: {0}")]
    LinkNotFound(LinkId),

    // ===== v0.6.0 (ADR 0010 §1) — the session tier =====
    /// Resolution by name failed (`--session work`, `session show work`). Keyed
    /// by name because that is what the user typed.
    #[error("session not found: {0}")]
    SessionNotFound(String),

    /// Two sessions may not share a name: `by_name` is how every verb resolves
    /// a `<ref>`, so a duplicate would make resolution a coin flip.
    #[error("a session named `{0}` already exists")]
    SessionExists(String),

    #[error("no session exists; create one with `xscore session new <name>` first")]
    NoSession,

    #[error("multiple sessions exist; specify `--session <name>`")]
    SessionAmbiguous,

    /// A native session IS this process's backend (its links are daemon-owned
    /// pipes). There is nothing to "stop managing" without destroying it, so
    /// `session detach` points at the truthful verb instead.
    #[error("session {0} is native — this process owns it; use `session kill`")]
    DetachNativeSession(SessionId),

    /// Invariant guard (arch.md §8): a child may only be bound to another child
    /// of the SAME session. Carries `what` ("pane", "window", "link") so the
    /// message names both sides of the rejected pairing.
    #[error("session {session} does not contain {what} {id}")]
    NotInSession {
        session: SessionId,
        what: &'static str,
        id: String,
    },

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

    // ===== v0.6.0 (ADR 0010 §D2) — tmux-backed sessions =====
    /// `session new --tmux` names the tmux session after the xscore session, so
    /// an existing tmux session of that name would be adopted by accident.
    /// `session attach` is the verb that says "this one already exists".
    #[error(
        "a tmux session named `{0}` already exists; use `xscore session attach {0}` to manage it"
    )]
    TmuxSessionExists(String),

    /// The host was reachable but the session is not there (arch.md §6). Rows
    /// are pruned for this and only this.
    #[error("tmux session `{session}` on {host} does not exist")]
    TmuxTargetGone { host: String, session: String },

    /// The remote end never answered: `ssh` failed to start or exited 255
    /// (auth, host key, network). Rows are NEVER pruned on this error — a
    /// laptop on a train would otherwise forget every remote session it
    /// manages (arch.md §6, "Failure semantics").
    #[error("tmux host {host} is unreachable: {reason}")]
    TmuxUnreachable { host: String, reason: String },

    /// A tmux session has exactly one link — its control client (arch.md §4,
    /// "One tmux session, one link"). A second link would be a second
    /// mechanism driving the same server session.
    #[error(
        "session {0} is tmux-backed and already has its control link; add panes/windows, not links"
    )]
    TmuxSessionHasOneLink(SessionId),

    /// ADR 0010 §D5: deleting a `Control` link would orphan every pane in the
    /// session into a buffer with no way back. Teardown is `session kill`
    /// (destroy) or `session detach` (stop managing).
    #[error("link {0} is a tmux control link; use `session kill` or `session detach` instead")]
    ControlLinkNotDeletable(LinkId),

    /// A control-mode operation was attempted on a session whose backend is
    /// not tmux. Internal misuse, not a user state.
    #[error("session {0} is not tmux-backed")]
    NotTmuxSession(SessionId),

    #[error("url parse error: {0}")]
    Url(String),

    /// The control client of a tmux session exited: the session ended, the
    /// server went away, or someone killed it. The rows may still be there, so
    /// this is what a pane operation reports instead of a socket error.
    #[error("tmux session `{0}` is gone: its control client exited (the tmux session ended, or the server went away)")]
    TmuxSessionGone(String),

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
