//! 终端注册表：`TerminalHandle` 与 `TerminalCore` 的一一绑定，以及字节泵任务。

use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::Mutex;

use crate::term::{TerminalCore, VtTerm};
use xscore::domain::{TerminalHandle, TerminalId};

/// 终端句柄与其 VT 网格的绑定。
pub struct Terminal {
    /// 后端终端句柄。
    pub handle: Arc<dyn TerminalHandle>,
    /// 该终端独占的 VT 状态。
    pub term: Arc<Mutex<VtTerm>>,
}

/// 当前进程中所有终端的注册表。
pub struct TerminalRegistry {
    terminals: HashMap<TerminalId, Terminal>,
}

impl TerminalRegistry {
    /// 创建空终端注册表。
    ///
    /// 参数/输出：无输入；返回空注册表。
    pub fn new() -> Self {
        Self {
            terminals: HashMap::new(),
        }
    }

    /// 注册一个终端：建立 TerminalCore，并启动把输出字节泵进网格的任务。
    ///
    /// 参数/输出：`self` 为输入输出；`handle`、`cols` 和 `rows` 为输入；返回终端 ID。
    pub fn register(
        &mut self,
        handle: Arc<dyn TerminalHandle>,
        cols: u16,
        rows: u16,
    ) -> TerminalId {
        let id = handle.id();
        let term = Arc::new(Mutex::new(VtTerm::new(cols, rows)));
        let mut rx = handle.subscribe();
        let pump = term.clone();
        tokio::spawn(async move {
            while let Ok(chunk) = rx.recv().await {
                pump.lock().await.feed(&chunk);
            }
        });
        self.terminals.insert(id, Terminal { handle, term });
        id
    }

    /// 按 ID 读取终端条目。
    ///
    /// 参数/输出：`self` 和 `id` 为输入；存在时返回条目引用。
    pub fn get(&self, id: TerminalId) -> Option<&Terminal> {
        self.terminals.get(&id)
    }

    /// 判断终端 ID 是否已注册。
    ///
    /// 参数/输出：`self` 和 `id` 为输入；返回是否存在。
    pub fn contains(&self, id: TerminalId) -> bool {
        self.terminals.contains_key(&id)
    }

    /// 从注册表移除终端并返回其条目。
    ///
    /// 参数/输出：`self` 为输入输出；`id` 为输入；返回被移除条目或 `None`。
    pub fn remove(&mut self, id: TerminalId) -> Option<Terminal> {
        self.terminals.remove(&id)
    }

    /// 返回已注册终端数量。
    ///
    /// 参数/输出：`self` 为输入；返回数量。
    pub fn len(&self) -> usize {
        self.terminals.len()
    }

    /// 判断注册表是否为空。
    ///
    /// 参数/输出：`self` 为输入；返回是否没有终端。
    pub fn is_empty(&self) -> bool {
        self.terminals.is_empty()
    }

    /// 迭代全部已注册终端 ID。
    ///
    /// 参数/输出：`self` 为输入；返回终端 ID 迭代器。
    pub fn ids(&self) -> impl Iterator<Item = TerminalId> + '_ {
        self.terminals.keys().copied()
    }
}

impl Default for TerminalRegistry {
    /// 创建空终端注册表。
    fn default() -> Self {
        Self::new()
    }
}
