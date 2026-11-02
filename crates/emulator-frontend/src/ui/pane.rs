//! pane 注册表：`PaneHandle` 与 `TerminalCore` 的一一绑定，以及字节泵任务。

use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::Mutex;

use emulator::domain::{PaneHandle, PaneId};
use emulator::term::{TerminalCore, VtTerm};

pub struct PaneEntry {
    pub handle: Arc<dyn PaneHandle>,
    pub term: Arc<Mutex<VtTerm>>,
}

pub struct PaneRegistry {
    panes: HashMap<PaneId, PaneEntry>,
}

impl PaneRegistry {
    pub fn new() -> Self {
        Self {
            panes: HashMap::new(),
        }
    }

    /// 注册一个 pane：建立 TerminalCore，并启动把输出字节泵进网格的任务。
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

    pub fn get(&self, id: PaneId) -> Option<&PaneEntry> {
        self.panes.get(&id)
    }

    pub fn contains(&self, id: PaneId) -> bool {
        self.panes.contains_key(&id)
    }

    pub fn remove(&mut self, id: PaneId) -> Option<PaneEntry> {
        self.panes.remove(&id)
    }

    pub fn len(&self) -> usize {
        self.panes.len()
    }

    pub fn ids(&self) -> impl Iterator<Item = PaneId> + '_ {
        self.panes.keys().copied()
    }
}
