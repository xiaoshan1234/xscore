//! 前端应用状态：pane 注册表、布局树与 tmux 会话元数据。
//!
//! 具体命令逻辑在 `commands`，这里只保留协调所需的注册表与最小访问器。

use std::collections::HashMap;
use std::sync::Arc;

use emulator::domain::{PaneHandle, PaneId};
use emulator::platform::tmux::TmuxControl;

use crate::ui::layout::Layout;
use crate::ui::pane::{PaneEntry, PaneRegistry};
use crate::ui::tmux::session::TmuxMeta;
use crate::ui::window_group::{GroupId, GroupKind, WindowGroup};

pub struct App {
    pub panes: PaneRegistry,
    pub layout: Layout,
    pub tmux: HashMap<PaneId, TmuxMeta>,
    pub tmux_controls: HashMap<String, Arc<TmuxControl>>,
    pub groups: HashMap<GroupId, WindowGroup>,
    pub session_groups: HashMap<String, GroupId>,
    pub default_group: GroupId,
}

impl App {
    pub fn new() -> Self {
        let default_group = WindowGroup::default_group();
        let mut groups = HashMap::new();
        groups.insert(default_group.id, default_group.clone());
        Self {
            panes: PaneRegistry::new(),
            layout: Layout::new(),
            tmux: HashMap::new(),
            tmux_controls: HashMap::new(),
            groups,
            session_groups: HashMap::new(),
            default_group: default_group.id,
        }
    }

    /// 注册一个本地 pane（shell/ssh/wsl），归入默认 group。
    pub fn register_local_pane(
        &mut self,
        handle: Arc<dyn PaneHandle>,
        cols: u16,
        rows: u16,
    ) -> PaneId {
        let id = self.panes.register(handle, cols, rows);
        self.add_pane_to_group(self.default_group, id);
        id
    }

    /// 注册一个 tmux pane，归入该 session 对应的 window group。
    pub fn register_tmux_pane(
        &mut self,
        handle: Arc<dyn PaneHandle>,
        cols: u16,
        rows: u16,
        session: &str,
    ) -> PaneId {
        let id = self.panes.register(handle, cols, rows);
        let group = self.ensure_tmux_group(session);
        self.add_pane_to_group(group, id);
        id
    }

    pub fn pane_entry(&self, id: PaneId) -> Option<&PaneEntry> {
        self.panes.get(id)
    }

    pub fn tmux_control(&self, session: &str) -> Option<Arc<TmuxControl>> {
        self.tmux_controls.get(session).cloned()
    }

    /// 每个 tmux session 对应一个 window group；已存在则复用。
    pub fn ensure_tmux_group(&mut self, session: &str) -> GroupId {
        if let Some(id) = self.session_groups.get(session) {
            return *id;
        }
        let group = WindowGroup::tmux_session(session);
        let id = group.id;
        self.groups.insert(id, group);
        self.session_groups.insert(session.to_string(), id);
        id
    }

    pub fn add_pane_to_group(&mut self, group: GroupId, pane: PaneId) {
        if let Some(group) = self.groups.get_mut(&group) {
            group.add_pane(pane);
        }
    }

    /// 按 GroupId（数字）或 group 名（`default` / session 名）解析。
    pub fn resolve_group(&self, spec: &str) -> Option<GroupId> {
        if let Ok(id) = spec.parse::<GroupId>() {
            if self.groups.contains_key(&id) {
                return Some(id);
            }
        }
        self.groups
            .values()
            .find(|group| group.name == spec)
            .map(|group| group.id)
    }

    /// tmux window group 对应的 session 名。
    pub fn group_session(&self, id: GroupId) -> Option<&str> {
        self.groups
            .get(&id)
            .filter(|group| group.kind == GroupKind::Tmux)
            .map(|group| group.name.as_str())
    }

    pub fn remove_pane_local(&mut self, id: PaneId) {
        self.panes.remove(id);
        self.tmux.remove(&id);
        self.layout.remove_pane(id);
        for group in self.groups.values_mut() {
            group.remove_pane(id);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_and_session_groups() {
        let mut app = App::new();
        assert_eq!(app.groups.len(), 1);
        assert_eq!(app.groups[&app.default_group].name, "default");

        let gid = app.ensure_tmux_group("work");
        assert_eq!(app.groups.len(), 2);
        assert_eq!(app.groups[&gid].kind, GroupKind::Tmux);
        assert_eq!(app.session_groups["work"], gid);
        assert_eq!(app.ensure_tmux_group("work"), gid);
        assert_eq!(app.resolve_group("work"), Some(gid));
        assert_eq!(app.resolve_group(&gid.to_string()), Some(gid));
        assert_eq!(app.group_session(gid), Some("work"));
    }
}
