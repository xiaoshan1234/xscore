//! L3 组合窗格树（CLI 模拟）：本地分屏树、tmux 镜像容器与两级焦点。

use xscore::domain::PaneId;

/// 分屏方向。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    /// 左右分屏。
    Horizontal,
    /// 上下分屏。
    Vertical,
}

/// 一个 tmux pane 在 window 内的几何：列偏移、行偏移、宽、高。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PaneRect {
    /// 对应 pane ID。
    pub id: PaneId,
    /// 相对容器左偏移。
    pub x: u16,
    /// 相对容器上偏移。
    pub y: u16,
    /// pane 列数。
    pub cols: u16,
    /// pane 行数。
    pub rows: u16,
}

/// tmux window 的不可拆分布局原子。内部 pane 只在这里，不进本地树。
#[derive(Debug, Clone)]
pub struct TmuxContainer {
    /// tmux window 句柄。
    pub window: String,
    /// window 列数。
    pub cols: u16,
    /// window 行数。
    pub rows: u16,
    /// window 内的 pane 几何。
    pub panes: Vec<PaneRect>,
    /// 容器当前活动的 pane。
    pub active: Option<PaneId>,
}

/// 本地布局树节点。
#[derive(Debug, Clone)]
pub enum Node {
    /// 单个本地 pane 叶节点。
    Leaf(PaneId),
    /// 二分屏分支。
    Branch {
        /// 分屏方向。
        direction: Direction,
        /// 左侧/上方节点占比。
        ratio: f32,
        /// 第一个子节点。
        left: Box<Node>,
        /// 第二个子节点。
        right: Box<Node>,
    },
    /// 不可拆分的 tmux window 镜像。
    TmuxMirror(TmuxContainer),
}

/// 本地分屏树及焦点状态。
#[derive(Debug, Default)]
pub struct Layout {
    root: Option<Node>,
    focused: Option<PaneId>,
}

impl Layout {
    /// 创建空布局。
    ///
    /// 参数/输出：无输入；返回空 [`Layout`]。
    pub fn new() -> Self {
        Self::default()
    }

    /// 返回当前聚焦的 pane。
    ///
    /// 参数/输出：`self` 为输入；返回焦点或 `None`。
    pub fn focused(&self) -> Option<PaneId> {
        self.focused
    }

    /// 本地 pane：第一个为根，后续作为聚焦节点兄弟（默认垂直拆分）。
    ///
    /// 参数/输出：`self` 为输入输出；`pane` 为输入；不返回值。
    pub fn add(&mut self, pane: PaneId) {
        self.split_focused(Direction::Vertical, pane);
    }

    /// 拆分聚焦节点，新 pane 为右/下兄弟。
    ///
    /// 参数/输出：`self` 为输入输出；`direction` 和 `pane` 为输入；不返回值。
    pub fn split_focused(&mut self, direction: Direction, pane: PaneId) {
        self.insert_node(direction, Node::Leaf(pane));
    }

    /// 插入一个 tmux 镜像容器（不可拆分布局原子）。
    ///
    /// 参数/输出：`self` 为输入输出；`container` 为输入；不返回值。
    pub fn add_mirror(&mut self, container: TmuxContainer) {
        self.insert_node(Direction::Vertical, Node::TmuxMirror(container));
    }

    /// 在指定 pane 上拆分，新 pane 为右/下兄弟。
    ///
    /// 参数/输出：`self` 为输入输出；`direction`、`target`、`pane` 为输入；
    /// 不返回值。
    pub fn split_pane(&mut self, direction: Direction, target: PaneId, pane: PaneId) {
        self.insert_node_at(direction, Some(target), Node::Leaf(pane));
    }

    /// 布局树中是否包含某个 pane。
    ///
    /// 参数/输出：`self` 和 `pane` 为输入；返回是否存在。
    pub fn contains(&self, pane: PaneId) -> bool {
        self.root
            .as_ref()
            .map(|root| contains_in(root, pane))
            .unwrap_or(false)
    }

    fn insert_node(&mut self, direction: Direction, node: Node) {
        let target = self.focused;
        self.insert_node_at(direction, target, node);
    }

    fn insert_node_at(&mut self, direction: Direction, target: Option<PaneId>, node: Node) {
        let focus = node_focus(&node);
        let root = match self.root.take() {
            None => {
                self.root = Some(node);
                self.focused = focus;
                return;
            }
            Some(root) => match target {
                Some(target) => split_at(root, target, direction, node),
                None => Node::Branch {
                    direction,
                    ratio: 0.5,
                    left: Box::new(root),
                    right: Box::new(node),
                },
            },
        };
        self.root = Some(root);
        self.focused = focus;
    }

    /// 本地叶子焦点。
    ///
    /// 参数/输出：`self` 为输入输出；`pane` 为输入；不返回值。
    pub fn focus(&mut self, pane: PaneId) {
        self.focused = Some(pane);
    }

    /// 容器内焦点：找到指定 window 的镜像并设置 active。
    ///
    /// 参数/输出：`self` 为输入输出；`window` 和 `pane` 为输入；返回是否找到。
    pub fn set_mirror_active(&mut self, window: &str, pane: PaneId) -> bool {
        let found = match self.root.as_mut() {
            Some(root) => set_mirror_active_in(root, window, pane),
            None => false,
        };
        if found {
            self.focused = Some(pane);
        }
        found
    }

    /// 从布局树移除指定 pane，并自动收缩空分支。
    ///
    /// 参数/输出：`self` 为输入输出；`pane` 为输入；不返回值。
    pub fn remove_pane(&mut self, pane: PaneId) {
        self.root = self.root.take().and_then(|root| remove_pane_in(root, pane));
        self.focused = self.root.as_ref().and_then(node_focus);
    }

    /// 从布局树移除指定 tmux window 镜像。
    ///
    /// 参数/输出：`self` 为输入输出；`window` 为输入；不返回值。
    pub fn remove_window(&mut self, window: &str) {
        self.root = self
            .root
            .take()
            .and_then(|root| remove_window_in(root, window));
        self.focused = self.root.as_ref().and_then(node_focus);
    }

    /// 将布局树渲染为缩进文本。
    ///
    /// 参数/输出：`self` 为输入；返回布局文本。
    pub fn render(&self) -> String {
        match &self.root {
            None => "(empty)".to_string(),
            Some(root) => {
                let mut out = String::new();
                render_node(root, 0, &mut out);
                while out.ends_with('\n') {
                    out.pop();
                }
                out
            }
        }
    }
}

fn node_focus(node: &Node) -> Option<PaneId> {
    match node {
        Node::Leaf(id) => Some(*id),
        Node::TmuxMirror(container) => container
            .active
            .or_else(|| container.panes.first().map(|rect| rect.id)),
        Node::Branch { left, .. } => node_focus(left),
    }
}

fn contains_in(node: &Node, pane: PaneId) -> bool {
    match node {
        Node::Leaf(id) => *id == pane,
        Node::TmuxMirror(container) => container.panes.iter().any(|rect| rect.id == pane),
        Node::Branch { left, right, .. } => contains_in(left, pane) || contains_in(right, pane),
    }
}

fn split_at(node: Node, target: PaneId, direction: Direction, new_node: Node) -> Node {
    match node {
        Node::Leaf(id) if id == target => Node::Branch {
            direction,
            ratio: 0.5,
            left: Box::new(Node::Leaf(id)),
            right: Box::new(new_node),
        },
        Node::TmuxMirror(container) if container.panes.iter().any(|rect| rect.id == target) => {
            Node::Branch {
                direction,
                ratio: 0.5,
                left: Box::new(Node::TmuxMirror(container)),
                right: Box::new(new_node),
            }
        }
        Node::Branch {
            direction: dir,
            ratio,
            left,
            right,
        } => Node::Branch {
            direction: dir,
            ratio,
            left: Box::new(split_at(*left, target, direction, new_node.clone())),
            right: Box::new(split_at(*right, target, direction, new_node)),
        },
        other => other,
    }
}

fn set_mirror_active_in(node: &mut Node, window: &str, pane: PaneId) -> bool {
    match node {
        Node::TmuxMirror(container) if container.window == window => {
            container.active = Some(pane);
            true
        }
        Node::Branch { left, right, .. } => {
            set_mirror_active_in(left, window, pane) || set_mirror_active_in(right, window, pane)
        }
        _ => false,
    }
}

fn remove_pane_in(node: Node, pane: PaneId) -> Option<Node> {
    match node {
        Node::Leaf(id) if id == pane => None,
        Node::TmuxMirror(mut container) => {
            container.panes.retain(|rect| rect.id != pane);
            if container.panes.is_empty() {
                None
            } else {
                Some(Node::TmuxMirror(container))
            }
        }
        Node::Branch {
            direction,
            ratio,
            left,
            right,
        } => {
            let new_left = remove_pane_in(*left, pane);
            let new_right = remove_pane_in(*right, pane);
            match (new_left, new_right) {
                (Some(l), Some(r)) => Some(Node::Branch {
                    direction,
                    ratio,
                    left: Box::new(l),
                    right: Box::new(r),
                }),
                (Some(l), None) => Some(l),
                (None, Some(r)) => Some(r),
                (None, None) => None,
            }
        }
        other => Some(other),
    }
}

fn remove_window_in(node: Node, window: &str) -> Option<Node> {
    match node {
        Node::TmuxMirror(container) if container.window == window => None,
        Node::Branch {
            direction,
            ratio,
            left,
            right,
        } => {
            let new_left = remove_window_in(*left, window);
            let new_right = remove_window_in(*right, window);
            match (new_left, new_right) {
                (Some(l), Some(r)) => Some(Node::Branch {
                    direction,
                    ratio,
                    left: Box::new(l),
                    right: Box::new(r),
                }),
                (Some(l), None) => Some(l),
                (None, Some(r)) => Some(r),
                (None, None) => None,
            }
        }
        other => Some(other),
    }
}

fn render_node(node: &Node, depth: usize, out: &mut String) {
    let indent = "  ".repeat(depth);
    match node {
        Node::Leaf(id) => out.push_str(&format!("{indent}P{id}\n")),
        Node::Branch {
            direction,
            ratio,
            left,
            right,
        } => {
            let marker = match direction {
                Direction::Horizontal => 'h',
                Direction::Vertical => 'v',
            };
            out.push_str(&format!("{indent}{marker}: {ratio:.2}\n"));
            render_node(left, depth + 1, out);
            render_node(right, depth + 1, out);
        }
        Node::TmuxMirror(container) => {
            out.push_str(&format!(
                "{indent}window {}({}*{}):\n",
                container.window, container.cols, container.rows
            ));
            for rect in &container.panes {
                let star = if container.active == Some(rect.id) {
                    "*"
                } else {
                    ""
                };
                out.push_str(&format!(
                    "{indent}  P{}({},{},{},{}){}\n",
                    rect.id, rect.y, rect.x, rect.cols, rect.rows, star
                ));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(id: u64, x: u16, y: u16, cols: u16, rows: u16) -> PaneRect {
        PaneRect {
            id: PaneId::new(id),
            x,
            y,
            cols,
            rows,
        }
    }

    fn mirror(window: &str, pane_ids: &[u64]) -> TmuxContainer {
        TmuxContainer {
            window: window.to_string(),
            cols: 50,
            rows: 50,
            panes: pane_ids
                .iter()
                .enumerate()
                .map(|(index, id)| {
                    if index == 0 {
                        rect(*id, 0, 0, 25, 50)
                    } else {
                        rect(*id, 25, 0, 25, 50)
                    }
                })
                .collect(),
            active: pane_ids.first().map(|id| PaneId::new(*id)),
        }
    }

    #[test]
    fn first_pane_becomes_root() {
        let mut layout = Layout::new();
        layout.add(PaneId::new(1));
        assert_eq!(layout.render(), "P1");
    }

    #[test]
    fn add_splits_vertically() {
        let mut layout = Layout::new();
        layout.add(PaneId::new(1));
        layout.add(PaneId::new(2));
        assert_eq!(layout.render(), "v: 0.50\n  P1\n  P2");
    }

    #[test]
    fn split_pane_targets_a_specific_pane() {
        let mut layout = Layout::new();
        layout.add(PaneId::new(1));
        layout.add(PaneId::new(2));
        layout.split_pane(Direction::Horizontal, PaneId::new(1), PaneId::new(3));
        assert_eq!(layout.render(), "v: 0.50\n  h: 0.50\n    P1\n    P3\n  P2");
        assert!(layout.contains(PaneId::new(3)));
        assert!(!layout.contains(PaneId::new(9)));
    }

    #[test]
    fn mirror_shows_window_and_pane_geometry() {
        let mut layout = Layout::new();
        layout.add_mirror(mirror("@1", &[10, 11]));
        assert_eq!(
            layout.render(),
            "window @1(50*50):\n  P10(0,0,25,50)*\n  P11(0,25,25,50)"
        );
    }

    #[test]
    fn mirror_is_atomic_and_not_split() {
        let mut layout = Layout::new();
        layout.add_mirror(mirror("@1", &[10, 11]));
        layout.add(PaneId::new(20));
        assert_eq!(
            layout.render(),
            "v: 0.50\n  window @1(50*50):\n    P10(0,0,25,50)*\n    P11(0,25,25,50)\n  P20"
        );
    }

    #[test]
    fn mirror_active_pane_is_two_level_focus() {
        let mut layout = Layout::new();
        layout.add_mirror(mirror("@1", &[10, 11]));
        assert!(layout.set_mirror_active("@1", PaneId::new(11)));
        assert_eq!(layout.focused(), Some(PaneId::new(11)));
        assert_eq!(
            layout.render(),
            "window @1(50*50):\n  P10(0,0,25,50)\n  P11(0,25,25,50)*"
        );
    }

    #[test]
    fn remove_pane_shrinks_branch() {
        let mut layout = Layout::new();
        layout.add(PaneId::new(1));
        layout.split_focused(Direction::Horizontal, PaneId::new(2));
        layout.remove_pane(PaneId::new(2));
        assert_eq!(layout.render(), "P1");
    }

    #[test]
    fn remove_window_removes_mirror() {
        let mut layout = Layout::new();
        layout.add_mirror(mirror("@1", &[10, 11]));
        layout.add(PaneId::new(20));
        layout.remove_window("@1");
        assert_eq!(layout.render(), "P20");
    }
}
