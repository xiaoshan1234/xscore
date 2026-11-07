//! WindowGroup：一组窗口（本地会话 / tmux window）的容器，带唯一 GroupId。

use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::{anyhow, Result};
use xscore::domain::TerminalId;

/// 全局唯一的 window group 标识。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct GroupId(u64);

impl GroupId {
    /// 分配全局唯一的下一个 group ID。
    ///
    /// 参数/输出：无输入；返回新 ID。
    pub fn next() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        Self(NEXT.fetch_add(1, Ordering::Relaxed) + 1)
    }

    /// 读取 group ID 的整数形式。
    ///
    /// 参数/输出：`self` 为输入；返回 ID 数值。
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

/// window group 的类别。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GroupKind {
    /// 默认的非 tmux 会话组。
    Default,
    /// 对应一个 tmux session 的组。
    Tmux,
}

/// 窗口类别。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum WindowKind {
    /// tmux 窗口。
    Tmux,
    /// 本地手动创建窗口。
    Manual,
}

/// 由 group、类别和序号组成的窗口标识。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct WindowId {
    /// 所属 group。
    pub group: GroupId,
    /// 窗口类别。
    pub kind: WindowKind,
    /// 组内窗口序号。
    pub index: u64,
}

impl WindowId {
    /// 构造 tmux window ID。
    ///
    /// 参数/输出：`group` 和 `index` 为输入；返回 tmux 类别 ID。
    pub fn tmux(group: GroupId, index: u64) -> Self {
        Self {
            group,
            kind: WindowKind::Tmux,
            index,
        }
    }

    /// 构造手动 window ID。
    ///
    /// 参数/输出：`group` 和 `index` 为输入；返回 manual 类别 ID。
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
///
/// 参数/输出：`s` 为输入；成功时返回 [`WindowId`]，格式非法时返回错误。
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

/// 一个 window group 及其 pane 成员。
#[derive(Debug, Clone)]
pub struct WindowGroup {
    /// group 唯一标识。
    pub id: GroupId,
    /// group 显示名。
    pub name: String,
    /// group 类别。
    pub kind: GroupKind,
    /// 属于该 group 的 pane。
    pub panes: Vec<TerminalId>,
}

impl WindowGroup {
    /// 创建空默认 group。
    ///
    /// 参数/输出：无输入；返回新的默认组。
    pub fn default_group() -> Self {
        Self {
            id: GroupId::next(),
            name: "default".to_string(),
            kind: GroupKind::Default,
            panes: Vec::new(),
        }
    }

    /// 创建与 tmux session 关联的空 group。
    ///
    /// 参数/输出：`session` 为输入；返回新的 tmux group。
    pub fn tmux_session(session: &str) -> Self {
        Self {
            id: GroupId::next(),
            name: session.to_string(),
            kind: GroupKind::Tmux,
            panes: Vec::new(),
        }
    }

    /// 将一个 pane 加入 group，重复加入会被忽略。
    ///
    /// 参数/输出：`self` 为输入输出；`pane` 为输入；不返回值。
    pub fn add_pane(&mut self, pane: TerminalId) {
        if !self.panes.contains(&pane) {
            self.panes.push(pane);
        }
    }

    /// 从 group 中移除一个 pane。
    ///
    /// 参数/输出：`self` 为输入输出；`pane` 为输入；不返回值。
    pub fn remove_pane(&mut self, pane: TerminalId) {
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

        group.add_pane(TerminalId::new(1));
        group.add_pane(TerminalId::new(2));
        group.add_pane(TerminalId::new(1));
        assert_eq!(group.panes.len(), 2);

        group.remove_pane(TerminalId::new(1));
        assert_eq!(group.panes, vec![TerminalId::new(2)]);
    }
}
