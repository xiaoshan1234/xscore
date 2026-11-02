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

pub struct App {
    pub panes: PaneRegistry,
    pub layout: Layout,
    pub tmux: HashMap<PaneId, TmuxMeta>,
    pub tmux_controls: HashMap<String, Arc<TmuxControl>>,
}

impl App {
    pub fn new() -> Self {
        Self {
            panes: PaneRegistry::new(),
            layout: Layout::new(),
            tmux: HashMap::new(),
            tmux_controls: HashMap::new(),
        }
    }

    pub fn register_pane(&mut self, handle: Arc<dyn PaneHandle>, cols: u16, rows: u16) -> PaneId {
        self.panes.register(handle, cols, rows)
    }

    pub fn pane_entry(&self, id: PaneId) -> Option<&PaneEntry> {
        self.panes.get(id)
    }

    pub fn tmux_control(&self, session: &str) -> Option<Arc<TmuxControl>> {
        self.tmux_controls.get(session).cloned()
    }

    pub fn remove_pane_local(&mut self, id: PaneId) {
        self.panes.remove(id);
        self.tmux.remove(&id);
        self.layout.remove_pane(id);
    }
}
