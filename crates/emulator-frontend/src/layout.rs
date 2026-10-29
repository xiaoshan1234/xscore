//! L3 组合窗格树（CLI 模拟）：本地分屏树与焦点。

use emulator::domain::PaneId;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Horizontal,
    Vertical,
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

    /// 第一个 pane 成为根；后续 pane 作为聚焦叶子的兄弟（默认垂直拆分）。
    pub fn add(&mut self, pane: PaneId) {
        self.split_focused(Direction::Vertical, pane);
    }

    /// 在聚焦叶子处按方向拆分，新 pane 为右/下兄弟。
    pub fn split_focused(&mut self, direction: Direction, pane: PaneId) {
        let root = match self.root.take() {
            None => {
                self.root = Some(Node::Leaf(pane));
                self.focused = Some(pane);
                return;
            }
            Some(root) => match self.focused {
                Some(focused) => replace_leaf(root, focused, direction, pane),
                None => Node::Branch {
                    direction,
                    ratio: 0.5,
                    left: Box::new(root),
                    right: Box::new(Node::Leaf(pane)),
                },
            },
        };
        self.root = Some(root);
        self.focused = Some(pane);
    }

    pub fn focus(&mut self, pane: PaneId) {
        self.focused = Some(pane);
    }

    pub fn render(&self) -> String {
        match &self.root {
            None => "(empty)".to_string(),
            Some(root) => render_node(root),
        }
    }
}

fn replace_leaf(node: Node, target: PaneId, split_dir: Direction, pane: PaneId) -> Node {
    match node {
        Node::Leaf(id) if id == target => Node::Branch {
            direction: split_dir,
            ratio: 0.5,
            left: Box::new(Node::Leaf(id)),
            right: Box::new(Node::Leaf(pane)),
        },
        Node::Branch {
            direction,
            ratio,
            left,
            right,
        } => Node::Branch {
            direction,
            ratio,
            left: Box::new(replace_leaf(*left, target, split_dir, pane)),
            right: Box::new(replace_leaf(*right, target, split_dir, pane)),
        },
        other => other,
    }
}

fn render_node(node: &Node) -> String {
    match node {
        Node::Leaf(id) => format!("P{id}"),
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

    #[test]
    fn first_pane_becomes_root() {
        let mut layout = Layout::new();
        layout.add(PaneId::new(1));
        assert_eq!(layout.render(), "P1");
        assert_eq!(layout.focused(), Some(PaneId::new(1)));
    }

    #[test]
    fn add_splits_vertically() {
        let mut layout = Layout::new();
        layout.add(PaneId::new(1));
        layout.add(PaneId::new(2));
        assert_eq!(layout.render(), "[P1 / P2]");
        assert_eq!(layout.focused(), Some(PaneId::new(2)));
    }

    #[test]
    fn split_horizontal_then_focus() {
        let mut layout = Layout::new();
        layout.add(PaneId::new(1));
        layout.split_focused(Direction::Horizontal, PaneId::new(2));
        assert_eq!(layout.render(), "[P1 | P2]");
        layout.focus(PaneId::new(1));
        layout.split_focused(Direction::Vertical, PaneId::new(3));
        assert_eq!(layout.render(), "[[P1 / P3] | P2]");
    }
}
