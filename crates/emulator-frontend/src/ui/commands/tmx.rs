//! tmx attach：URL 化的 tmux 会话接入。

use std::sync::Arc;

use anyhow::Result;
use tokio::sync::Mutex;

use crate::cli::{build_tmx_config, TmxAttachArgs};
use crate::ui::app::App;
use crate::ui::commands::tmux::attach_session;
use crate::ui::render::Output;

/// 解析 tmux URL 并接入目标 session。
///
/// 参数/输出：`app` 为输入输出；`args` 和 `app_arc` 为输入；返回 attach 摘要。
pub async fn attach(
    app: &mut App,
    args: &TmxAttachArgs,
    app_arc: Arc<Mutex<App>>,
) -> Result<Output> {
    let (socket, host, session) = build_tmx_config(args)?;
    attach_session(app, &host, &socket, &session, app_arc).await
}
