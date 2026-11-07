//! tmx attach：URL 化的 tmux 会话接入。

use std::sync::Arc;

use anyhow::{anyhow, Result};
use tokio::sync::Mutex;
use xsc_protocol::{parse_conversation_url, parse_ssh_target, ConversationSpec, TmxAttachRequest};

use xscore::platform::tmux::{TmuxHostConfig, TmuxSocket};

use crate::app::App;
use crate::commands::tmux::attach_session;
use crate::render::Output;

/// 解析 tmux URL 并接入目标 session。
///
/// 参数/输出：`app` 为输入输出；`args` 和 `app_arc` 为输入；返回 attach 摘要。
pub async fn attach(
    app: &mut App,
    args: &TmxAttachRequest,
    app_arc: Arc<Mutex<App>>,
) -> Result<Output> {
    let (socket, host, session) = build_tmx_config(args)?;
    attach_session(app, &host, &socket, &session, app_arc).await
}

fn build_tmx_config(args: &TmxAttachRequest) -> Result<(TmuxSocket, TmuxHostConfig, String)> {
    let (socket, session) = parse_tmux_url(&args.url)?;
    let host = match (&args.ssh, &args.wsl) {
        (Some(ssh_url), None) => {
            let ConversationSpec::Ssh {
                target,
                identity,
                password,
                ..
            } = parse_conversation_url(ssh_url)?
            else {
                return Err(anyhow!("--ssh requires an ssh:// URL"));
            };
            let (user, host, port) = parse_ssh_target(&target)?;
            TmuxHostConfig::Ssh {
                user,
                host,
                port,
                identity,
                password,
            }
        }
        (None, Some(wsl_url)) => {
            let ConversationSpec::Wsl { args, .. } = parse_conversation_url(wsl_url)? else {
                return Err(anyhow!("--wsl requires a wsl:// URL"));
            };
            TmuxHostConfig::Wsl { args }
        }
        (Some(_), Some(_)) => return Err(anyhow!("--ssh and --wsl are mutually exclusive")),
        (None, None) => TmuxHostConfig::Local,
    };
    Ok((socket, host, session))
}

fn parse_tmux_url(url: &str) -> Result<(TmuxSocket, String)> {
    let rest = url
        .strip_prefix("tmux://")
        .ok_or_else(|| anyhow!("expected tmux://... URL, got {url}"))?;
    let (socket, session) = match rest.rsplit_once('/') {
        Some((socket, session)) => (socket, session),
        None => ("", rest),
    };
    if session.is_empty() {
        return Err(anyhow!(
            "tmux URL requires a session: tmux://<socket>/<session>"
        ));
    }
    let socket = if socket.is_empty() {
        TmuxSocket::Default
    } else if socket.contains('/') {
        TmuxSocket::Path(socket.to_string())
    } else {
        TmuxSocket::Named(socket.to_string())
    };
    Ok((socket, session.to_string()))
}
