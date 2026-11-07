//! 前端应用状态：pane 注册表、布局树、window group 与 window id。

use std::collections::HashMap;
use std::sync::Arc;

use xscore::domain::{PaneHandle, PaneId};
use xscore::platform::tmux::TmuxControl;

use crate::app::{GroupId, GroupKind, PaneEntry, PaneRegistry, WindowGroup, WindowId};
use crate::layout::Layout;
use crate::logging::LoggerRegistry;
use crate::tmux::session::TmuxMeta;

/// CLI 前端的全局应用状态。
pub struct App {
    /// 所有已注册 pane。
    pub panes: PaneRegistry,
    /// 本地布局树。
    pub layout: Layout,
    /// tmux pane 的会话和几何元数据。
    pub tmux: HashMap<PaneId, TmuxMeta>,
    /// 按 tmux session 保存的控制连接。
    pub tmux_controls: HashMap<String, Arc<TmuxControl>>,
    /// 全部 window group。
    pub groups: HashMap<GroupId, WindowGroup>,
    /// tmux session 到 group 的映射。
    pub session_groups: HashMap<String, GroupId>,
    /// pane 到 group 的映射。
    pub pane_groups: HashMap<PaneId, GroupId>,
    /// pane 到 window 的映射。
    pub pane_windows: HashMap<PaneId, WindowId>,
    /// 每个 group 下一个可用的 manual window 序号。
    pub next_manual_window: HashMap<GroupId, u64>,
    /// 默认 group ID。
    pub default_group: GroupId,
    /// daemon 中运行的 pane 输出 logger。
    pub loggers: LoggerRegistry,
}

impl App {
    /// 创建带默认 group 的空应用状态。
    ///
    /// 参数/输出：无输入；返回新的 [`App`]。
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
            pane_groups: HashMap::new(),
            pane_windows: HashMap::new(),
            next_manual_window: HashMap::new(),
            default_group: default_group.id,
            loggers: LoggerRegistry::new(),
        }
    }

    /// 注册一个 pane，并归入 group 里的一个新 manual window。
    ///
    /// 参数/输出：`self` 为输入输出；`handle`、`cols`、`rows`、`group` 为输入；
    /// 返回新 pane ID。
    pub fn register_pane_in_group(
        &mut self,
        handle: Arc<dyn PaneHandle>,
        cols: u16,
        rows: u16,
        group: GroupId,
    ) -> PaneId {
        let id = self.panes.register(handle, cols, rows);
        self.add_pane_to_group(group, id);
        let window = self.alloc_manual_window(group);
        self.pane_windows.insert(id, window);
        id
    }

    /// 注册一个 pane，并归入指定的 window。
    ///
    /// 参数/输出：`self` 为输入输出；`handle`、`cols`、`rows`、`window` 为输入；
    /// 返回新 pane ID。
    pub fn register_pane_in_window(
        &mut self,
        handle: Arc<dyn PaneHandle>,
        cols: u16,
        rows: u16,
        window: WindowId,
    ) -> PaneId {
        let id = self.panes.register(handle, cols, rows);
        self.add_pane_to_group(window.group, id);
        self.pane_windows.insert(id, window);
        id
    }

    /// 注册一个 tmux pane，归入该 session 对应 group 的 tmux window。
    ///
    /// 参数/输出：`self` 为输入输出；`handle`、`cols`、`rows`、`session` 和
    /// `window_handle` 为输入；返回新 pane ID。
    pub fn register_tmux_pane(
        &mut self,
        handle: Arc<dyn PaneHandle>,
        cols: u16,
        rows: u16,
        session: &str,
        window_handle: &str,
    ) -> PaneId {
        let id = self.panes.register(handle, cols, rows);
        let group = self.ensure_tmux_group(session);
        self.add_pane_to_group(group, id);
        let index = tmux_window_index(window_handle);
        self.pane_windows.insert(id, WindowId::tmux(group, index));
        id
    }

    /// 同步 pane 的 tmux window id（tmux window 序号变化时调用）。
    ///
    /// 参数/输出：`self` 为输入输出；`id`、`session`、`window_handle` 为输入；
    /// 不返回值。
    pub fn update_tmux_window(&mut self, id: PaneId, session: &str, window_handle: &str) {
        if let Some(group) = self.session_groups.get(session).copied() {
            let index = tmux_window_index(window_handle);
            self.pane_windows.insert(id, WindowId::tmux(group, index));
        }
    }

    fn alloc_manual_window(&mut self, group: GroupId) -> WindowId {
        let next = self.next_manual_window.entry(group).or_insert(0);
        let index = *next;
        *next += 1;
        WindowId::manual(group, index)
    }

    /// 按 pane ID 读取注册条目。
    ///
    /// 参数/输出：`self` 和 `id` 为输入；存在时返回条目引用。
    pub fn pane_entry(&self, id: PaneId) -> Option<&PaneEntry> {
        self.panes.get(id)
    }

    /// 按 session 名读取 tmux 控制连接。
    ///
    /// 参数/输出：`self` 和 `session` 为输入；存在时返回共享控制连接。
    pub fn tmux_control(&self, session: &str) -> Option<Arc<TmuxControl>> {
        self.tmux_controls.get(session).cloned()
    }

    /// 每个 tmux session 对应一个 window group；已存在则复用。
    ///
    /// 参数/输出：`self` 为输入输出；`session` 为输入；返回对应 group ID。
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

    /// 将 pane 加入 group 并更新反向映射。
    ///
    /// 参数/输出：`self` 为输入输出；`group` 和 `pane` 为输入；不返回值。
    pub fn add_pane_to_group(&mut self, group: GroupId, pane: PaneId) {
        if let Some(group) = self.groups.get_mut(&group) {
            group.add_pane(pane);
        }
        self.pane_groups.insert(pane, group);
    }

    /// 读取 pane 所属 group。
    ///
    /// 参数/输出：`self` 和 `id` 为输入；存在时返回 group ID。
    pub fn pane_group(&self, id: PaneId) -> Option<GroupId> {
        self.pane_groups.get(&id).copied()
    }

    /// 读取 pane 所属 window。
    ///
    /// 参数/输出：`self` 和 `id` 为输入；存在时返回 window ID。
    pub fn pane_window(&self, id: PaneId) -> Option<WindowId> {
        self.pane_windows.get(&id).copied()
    }

    /// 读取 group 显示名。
    ///
    /// 参数/输出：`self` 和 `id` 为输入；存在时返回名称引用。
    pub fn group_name(&self, id: GroupId) -> Option<&str> {
        self.groups.get(&id).map(|group| group.name.as_str())
    }

    /// 按 GroupId（数字）或 group 名（`default` / session 名）解析。
    ///
    /// 参数/输出：`self` 和 `spec` 为输入；匹配时返回 group ID。
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
    ///
    /// 参数/输出：`self` 和 `id` 为输入；tmux group 存在时返回 session 名。
    pub fn group_session(&self, id: GroupId) -> Option<&str> {
        self.groups
            .get(&id)
            .filter(|group| group.kind == GroupKind::Tmux)
            .map(|group| group.name.as_str())
    }

    /// 从本地应用状态中移除 pane 及其全部映射。
    ///
    /// 参数/输出：`self` 为输入输出；`id` 为输入；不返回值。
    pub fn remove_pane_local(&mut self, id: PaneId) {
        self.panes.remove(id);
        self.tmux.remove(&id);
        self.layout.remove_pane(id);
        for group in self.groups.values_mut() {
            group.remove_pane(id);
        }
        self.pane_groups.remove(&id);
        self.pane_windows.remove(&id);
        self.loggers.remove_pane(id);
    }
}

impl Default for App {
    /// Create the default application state.
    fn default() -> Self {
        Self::new()
    }
}

fn tmux_window_index(handle: &str) -> u64 {
    handle.trim_start_matches('@').parse().unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tmux_window_index_parses_handle() {
        assert_eq!(tmux_window_index("@0"), 0);
        assert_eq!(tmux_window_index("@3"), 3);
        assert_eq!(tmux_window_index("not-a-window"), 0);
    }

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
