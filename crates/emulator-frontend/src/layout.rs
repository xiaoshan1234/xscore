//! L3 组合窗格树（CLI 模拟）：本地分屏树、tmux 镜像容器与两级焦点。

use emulator::domain::PaneId;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Horizontal,
    Vertical,
}

/// tmux window 的不可拆分布局原子。内部 pane 只在这里，不进本地树。
#[derive(Debug, Clone)]
pub struct TmuxContainer {
    pub window: String,
    pub panes: Vec<PaneId>,
    pub active: Option<PaneId>,
}

#[derive(Debug, Clone)]
pub enum Node {
    Leaf(PaneId),
    Branch {
        direction: Direction,
        ratio: f32,
        left: Box<Node>,
        right: Box<Node>,
    },
    TmuxMirror(TmuxContainer),
}

#[derive(Debug, Default)]
pub struct Layout {
    root: Option<Node>,
    focused: Option<PaneId>,
}

impl Layout {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn focused(&self) -> Option<PaneId> {
        self.focused
    }

    /// 本地 pane：第一个为根，后续作为聚焦节点兄弟（默认垂直拆分）。
    pub fn add(&mut self, pane: PaneId) {
        self.split_focused(Direction::Vertical, pane);
    }

    /// 拆分聚焦节点，新 pane 为右/下兄弟。
    pub fn split_focused(&mut self, direction: Direction, pane: PaneId) {
        self.insert_node(direction, Node::Leaf(pane));
    }

    /// 插入一个 tmux 镜像容器（不可拆分布局原子）。
    pub fn add_mirror(&mut self, container: TmuxContainer) {
        self.insert_node(Direction::Vertical, Node::TmuxMirror(container));
    }

    fn insert_node(&mut self, direction: Direction, node: Node) {
        let focus = node_focus(&node);
        let root = match self.root.take() {
            None => {
                self.root = Some(node);
                self.focused = focus;
                return;
            }
            Some(root) => match self.focused {
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
    pub fn focus(&mut self, pane: PaneId) {
        self.focused = Some(pane);
    }

    /// 容器内焦点：找到指定 window 的镜像并设置 active。
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

    pub fn add_pane_to_window(&mut self, window: &str, pane: PaneId) -> bool {
        match self.root.as_mut() {
            Some(root) => add_pane_to_window_in(root, window, pane),
            None => false,
        }
    }

    pub fn remove_pane(&mut self, pane: PaneId) {
        self.root = self.root.take().and_then(|root| remove_pane_in(root, pane));
        self.focused = self.root.as_ref().and_then(node_focus);
    }

    pub fn remove_window(&mut self, window: &str) {
        self.root = self
            .root
            .take()
            .and_then(|root| remove_window_in(root, window));
        self.focused = self.root.as_ref().and_then(node_focus);
    }

    pub fn render(&self) -> String {
        match &self.root {
            None => "(empty)".to_string(),
            Some(root) => render_node(root),
        }
    }
}

fn node_focus(node: &Node) -> Option<PaneId> {
    match node {
        Node::Leaf(id) => Some(*id),
        Node::TmuxMirror(container) => container
            .active
            .or_else(|| container.panes.first().copied()),
        Node::Branch { left, .. } => node_focus(left),
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
        Node::TmuxMirror(container) if container.panes.contains(&target) => Node::Branch {
            direction,
            ratio: 0.5,
            left: Box::new(Node::TmuxMirror(container)),
            right: Box::new(new_node),
        },
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

fn add_pane_to_window_in(node: &mut Node, window: &str, pane: PaneId) -> bool {
    match node {
        Node::TmuxMirror(container) if container.window == window => {
            if !container.panes.contains(&pane) {
                container.panes.push(pane);
            }
            true
        }
        Node::Branch { left, right, .. } => {
            add_pane_to_window_in(left, window, pane) || add_pane_to_window_in(right, window, pane)
        }
        _ => false,
    }
}

fn remove_pane_in(node: Node, pane: PaneId) -> Option<Node> {
    match node {
        Node::Leaf(id) if id == pane => None,
        Node::TmuxMirror(mut container) => {
            container.panes.retain(|p| *p != pane);
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

fn render_node(node: &Node) -> String {
    match node {
        Node::Leaf(id) => format!("P{id}"),
        Node::TmuxMirror(container) => {
            let panes: Vec<String> = container
                .panes
                .iter()
                .map(|id| {
                    if Some(*id) == container.active {
                        format!("P{id}*")
                    } else {
                        format!("P{id}")
                    }
                })
                .collect();
            format!("{}[{}]", container.window, panes.join(","))
        }
        Node::Branch {
            direction,
            left,
            right,
            ..
        } => match direction {
            Direction::Horizontal => format!("[{} | {}]", render_node(left), render_node(right)),
            Direction::Vertical => format!("[{} / {}]", render_node(left), render_node(right)),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mirror(window: &str, panes: &[u64]) -> TmuxContainer {
        TmuxContainer {
            window: window.to_string(),
            panes: panes.iter().map(|n| PaneId::new(*n)).collect(),
            active: panes.first().map(|n| PaneId::new(*n)),
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
        assert_eq!(layout.render(), "[P1 / P2]");
    }

    #[test]
    fn mirror_is_atomic_and_not_split() {
        let mut layout = Layout::new();
        layout.add_mirror(mirror("@1", &[10, 11]));
        layout.add(PaneId::new(20));
        assert_eq!(layout.render(), "[@1[P10*,P11] / P20]");
    }

    #[test]
    fn mirror_active_pane_is_two_level_focus() {
        let mut layout = Layout::new();
        layout.add_mirror(mirror("@1", &[10, 11]));
        assert!(layout.set_mirror_active("@1", PaneId::new(11)));
        assert_eq!(layout.render(), "@1[P10,P11*]");
        assert_eq!(layout.focused(), Some(PaneId::new(11)));
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

    #[test]
    fn add_pane_to_window() {
        let mut layout = Layout::new();
        layout.add_mirror(mirror("@1", &[10]));
        assert!(layout.add_pane_to_window("@1", PaneId::new(11)));
        assert_eq!(layout.render(), "@1[P10*,P11]");
    }
}
