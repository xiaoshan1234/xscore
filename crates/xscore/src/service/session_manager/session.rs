// service/session_manager/session.rs (v0.6.0 — the Session tier, ADR 0010 §1)
//
// A session is the workspace that owns windows, panes and links. It carries no
// child lists on purpose: every child stores `session_id`, so there is exactly
// one copy of the association and `session kill` can find its children by
// scanning the registries (arch.md §1, §8).
//
// The backend — not the link — says whether tmux is involved, which is the
// structural point of ADR 0010: "does this go through tmux?" is a property of
// the workspace, and a link may no longer disagree with it.

use crate::domain::model::SessionInfo;

#[derive(Debug)]
pub struct Session {
    pub info: SessionInfo,
}

impl Session {
    pub fn new(info: SessionInfo) -> Self {
        Self { info }
    }
}
