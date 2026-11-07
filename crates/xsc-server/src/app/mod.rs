//! 应用状态：全局 App、pane 注册表与 window/group 标识。

mod group;
mod pane;
mod state;

pub use group::{parse_window_id, GroupId, GroupKind, WindowGroup, WindowId, WindowKind};
pub use pane::{PaneEntry, PaneRegistry};
pub use state::App;
