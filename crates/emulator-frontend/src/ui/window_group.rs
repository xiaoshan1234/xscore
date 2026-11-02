//! WindowGroup：一组窗口（本地会话 / tmux window）的容器，带唯一 GroupId。

use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::{anyhow, Result};
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum WindowKind {
    Tmux,
    Manual,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct WindowId {
    pub group: GroupId,
    pub kind: WindowKind,
    pub index: u64,
}

impl WindowId {
    pub fn tmux(group: GroupId, index: u64) -> Self {
        Self {
            group,
            kind: WindowKind::Tmux,
            index,
        }
    }

    pub fn manual(group: GroupId, index: u64) -> Self {
        Self {
            group,
            kind: WindowKind::Manual,
            index,
        }
    }
}

impl fmt::Display for WindowId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let kind = match self.kind {
            WindowKind::Tmux => 'T',
            WindowKind::Manual => 'M',
        };
        let text = format!("G{}-{}{}", self.group, kind, self.index);
        f.pad(&text)
    }
}

/// 解析 `Gxx-Tyy` / `Gxx-Myy` 形式的 window id。
pub fn parse_window_id(s: &str) -> Result<WindowId> {
    let rest = s
        .strip_prefix('G')
        .ok_or_else(|| anyhow!("expected window id Gxx-Tyy / Gxx-Myy, got {s}"))?;
    let (group, rest) = rest
        .split_once('-')
        .ok_or_else(|| anyhow!("expected window id Gxx-Tyy / Gxx-Myy, got {s}"))?;
    let group: GroupId = group
        .parse()
        .map_err(|_| anyhow!("invalid group id in {s}"))?;
    let mut chars = rest.chars();
    let kind = match chars.next() {
        Some('T') => WindowKind::Tmux,
        Some('M') => WindowKind::Manual,
        _ => return Err(anyhow!("expected T or M in {s}")),
    };
    let index: u64 = chars
        .as_str()
        .parse()
        .map_err(|_| anyhow!("invalid window index in {s}"))?;
    Ok(WindowId { group, kind, index })
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
    fn window_id_display() {
        let id = WindowId::tmux("1".parse().unwrap(), 3);
        assert_eq!(id.to_string(), "G1-T3");
        assert_eq!(format!("{:<8}", id), "G1-T3   ");

        let manual = WindowId::manual("2".parse().unwrap(), 0);
        assert_eq!(manual.to_string(), "G2-M0");
    }

    #[test]
    fn parse_window_id_roundtrip() {
        assert_eq!(
            parse_window_id("G2-T3").unwrap(),
            WindowId::tmux("2".parse().unwrap(), 3)
        );
        assert_eq!(
            parse_window_id("G1-M0").unwrap(),
            WindowId::manual("1".parse().unwrap(), 0)
        );
        assert!(parse_window_id("2-T3").is_err());
        assert!(parse_window_id("G2-X3").is_err());
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
