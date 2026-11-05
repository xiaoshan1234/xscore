//! pane 注册表：`PaneHandle` 与 `TerminalCore` 的一一绑定，以及字节泵任务。

use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::Mutex;

use emulator::domain::{PaneHandle, PaneId};
use emulator::term::{TerminalCore, VtTerm};

/// pane 句柄与其终端网格的绑定。
pub struct PaneEntry {
    /// 后端 pane 句柄。
    pub handle: Arc<dyn PaneHandle>,
    /// 该 pane 独占的终端状态。
    pub term: Arc<Mutex<VtTerm>>,
}

/// 当前进程中所有 pane 的注册表。
pub struct PaneRegistry {
    panes: HashMap<PaneId, PaneEntry>,
}

impl PaneRegistry {
    /// 创建空 pane 注册表。
    ///
    /// 参数/输出：无输入；返回空注册表。
    pub fn new() -> Self {
        Self {
            panes: HashMap::new(),
        }
    }

    /// 注册一个 pane：建立 TerminalCore，并启动把输出字节泵进网格的任务。
    ///
    /// 参数/输出：`self` 为输入输出；`handle`、`cols` 和 `rows` 为输入；返回 pane ID。
    pub fn register(&mut self, handle: Arc<dyn PaneHandle>, cols: u16, rows: u16) -> PaneId {
        let id = handle.id();
        let term = Arc::new(Mutex::new(VtTerm::new(cols, rows)));
        let mut rx = handle.subscribe();
        let pump = term.clone();
        tokio::spawn(async move {
            while let Ok(chunk) = rx.recv().await {
                pump.lock().await.feed(&chunk);
            }
        });
        self.panes.insert(id, PaneEntry { handle, term });
        id
    }

    /// 按 ID 读取 pane 条目。
    ///
    /// 参数/输出：`self` 和 `id` 为输入；存在时返回条目引用。
    pub fn get(&self, id: PaneId) -> Option<&PaneEntry> {
        self.panes.get(&id)
    }

    /// 判断 pane ID 是否已注册。
    ///
    /// 参数/输出：`self` 和 `id` 为输入；返回是否存在。
    pub fn contains(&self, id: PaneId) -> bool {
        self.panes.contains_key(&id)
    }

    /// 从注册表移除 pane 并返回其条目。
    ///
    /// 参数/输出：`self` 为输入输出；`id` 为输入；返回被移除条目或 `None`。
    pub fn remove(&mut self, id: PaneId) -> Option<PaneEntry> {
        self.panes.remove(&id)
    }

    /// 返回已注册 pane 数量。
    ///
    /// 参数/输出：`self` 为输入；返回数量。
    pub fn len(&self) -> usize {
        self.panes.len()
    }

    /// 迭代全部已注册 pane ID。
    ///
    /// 参数/输出：`self` 为输入；返回 pane ID 迭代器。
    pub fn ids(&self) -> impl Iterator<Item = PaneId> + '_ {
        self.panes.keys().copied()
    }
}
