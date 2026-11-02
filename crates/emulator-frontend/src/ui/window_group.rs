//! WindowGroup：一组窗口（本地会话 / tmux window）的容器，带唯一 GroupId。

use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};

use emulator::domain::PaneId;

/// 全局唯一的 window group 标识。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct GroupId(u64);

impl GroupId {
    pub fn next() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        Self(NEXT.fetch_add(1, Ordering::Relaxed) + 1)
    }

    pub fn as_u64(&self) -> u64 {
        self.0
    }
}

impl fmt::Display for GroupId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.0, f)
    }
}

impl std::str::FromStr for GroupId {
    type Err = std::num::ParseIntError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        s.parse::<u64>().map(GroupId)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GroupKind {
    Default,
    Tmux,
}

#[derive(Debug, Clone)]
pub struct WindowGroup {
    pub id: GroupId,
    pub name: String,
    pub kind: GroupKind,
    pub panes: Vec<PaneId>,
}

impl WindowGroup {
    pub fn default_group() -> Self {
        Self {
            id: GroupId::next(),
            name: "default".to_string(),
            kind: GroupKind::Default,
            panes: Vec::new(),
        }
    }

    pub fn tmux_session(session: &str) -> Self {
        Self {
            id: GroupId::next(),
            name: session.to_string(),
            kind: GroupKind::Tmux,
            panes: Vec::new(),
        }
    }

    pub fn add_pane(&mut self, pane: PaneId) {
        if !self.panes.contains(&pane) {
            self.panes.push(pane);
        }
    }

    pub fn remove_pane(&mut self, pane: PaneId) {
        self.panes.retain(|p| *p != pane);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn group_ids_are_unique() {
        assert_ne!(GroupId::next(), GroupId::next());
    }

    #[test]
    fn group_id_display_respects_width() {
        let id: GroupId = "1".parse().unwrap();
        assert_eq!(format!("{:<6}", id), "1     ");
        assert_eq!(format!("{:>6}", id), "     1");
    }

    #[test]
    fn groups_track_panes_without_duplicates() {
        let mut group = WindowGroup::default_group();
        assert_eq!(group.name, "default");
        assert_eq!(group.kind, GroupKind::Default);

        group.add_pane(PaneId::new(1));
        group.add_pane(PaneId::new(2));
        group.add_pane(PaneId::new(1));
        assert_eq!(group.panes.len(), 2);

        group.remove_pane(PaneId::new(1));
        assert_eq!(group.panes, vec![PaneId::new(2)]);
    }
}
