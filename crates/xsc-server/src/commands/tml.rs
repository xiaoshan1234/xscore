//! tml add / tml del：统一的 shell/ssh/wsl 对话与 tmux window/pane 管理。

use anyhow::{anyhow, Context, Result};

use xscore::domain::{Domain, LocalDomain, PaneConfig, PaneId, RemoteSshDomain};
use xscore::platform::tmux::quote_arg;

use crate::command::{parse_conversation_url, parse_ssh_target, ConversationSpec, TmlAddRequest};

use crate::app::App;
use crate::layout::Direction;
use crate::render::Output;
use crate::tmux::session::reconcile_session;
use crate::window_group::{GroupId, GroupKind, WindowId, WindowKind};

/// 按目标 group 类型创建本地对话、SSH/WSL pane 或 tmux window/pane。
///
/// 参数/输出：`app` 为输入输出；`args` 为输入；返回创建结果文本。
pub async fn add(app: &mut App, args: TmlAddRequest) -> Result<Output> {
    let group = match &args.group {
        Some(spec) => app
            .resolve_group(spec)
            .ok_or_else(|| anyhow!("group not found: {spec}"))?,
        None => app.default_group,
    };
    let is_tmux = app
        .groups
        .get(&group)
        .map(|g| g.kind == GroupKind::Tmux)
        .unwrap_or(false);
    if is_tmux {
        add_in_tmux(app, &args, group).await
    } else {
        add_local(app, args, group).await
    }
}

async fn add_local(app: &mut App, args: TmlAddRequest, group: GroupId) -> Result<Output> {
    let url = args
        .url
        .clone()
        .ok_or_else(|| anyhow!("--url is required for the default group"))?;
    let spec = parse_conversation_url(&url)?;
    let split = split_target(&args)?;
    if let Some((_, target)) = &split {
        if !app.layout.contains(*target) {
            return Err(anyhow!("pane not found in layout: {target}"));
        }
    }
    let target_window = match &split {
        Some((_, target)) => app
            .pane_window(*target)
            .filter(|window| window.kind == WindowKind::Manual),
        None => None,
    };
    let id = spawn(app, &spec, args.password, group, target_window).await?;
    match split {
        Some((direction, target)) => app.layout.split_pane(direction, target, id),
        None => app.layout.add(id),
    }

    let mut out = Output::new();
    out.line(format!(
        "pane: {id}  kind: {}  {}x{}",
        spec.scheme(),
        spec.cols(),
        spec.rows()
    ));
    Ok(out)
}

async fn add_in_tmux(app: &mut App, args: &TmlAddRequest, group: GroupId) -> Result<Output> {
    let session = app
        .group_session(group)
        .ok_or_else(|| anyhow!("group {group} is not a tmux session group"))?
        .to_string();
    let control = app
        .tmux_control(&session)
        .ok_or_else(|| anyhow!("tmux session control unavailable"))?;

    if let Some(raw) = &args.sph {
        let target = tmux_pane_handle(app, raw)?;
        control
            .send(&format!("split-window -t {} -h", quote_arg(&target)))
            .await?;
    } else if let Some(raw) = &args.spv {
        let target = tmux_pane_handle(app, raw)?;
        control
            .send(&format!("split-window -t {} -v", quote_arg(&target)))
            .await?;
    } else {
        control
            .send(&format!("new-window -t {}", quote_arg(&session)))
            .await?;
    }
    let _ = reconcile_session(app, &session, &control).await?;

    let mut out = Output::new();
    out.line(format!("created tmux window/pane in group: {session}"));
    Ok(out)
}

fn tmux_pane_handle(app: &App, raw: &str) -> Result<String> {
    let id = PaneId::new(raw.parse().context("pane id")?);
    app.tmux
        .get(&id)
        .map(|meta| meta.handle.clone())
        .ok_or_else(|| anyhow!("tmux pane not found: {id}"))
}

fn split_target(args: &TmlAddRequest) -> Result<Option<(Direction, PaneId)>> {
    if let Some(raw) = &args.sph {
        return Ok(Some((
            Direction::Horizontal,
            PaneId::new(raw.parse().context("pane id")?),
        )));
    }
    if let Some(raw) = &args.spv {
        return Ok(Some((
            Direction::Vertical,
            PaneId::new(raw.parse().context("pane id")?),
        )));
    }
    Ok(None)
}

async fn spawn(
    app: &mut App,
    spec: &ConversationSpec,
    password: Option<String>,
    group: GroupId,
    window: Option<WindowId>,
) -> Result<PaneId> {
    match spec {
        ConversationSpec::Shell {
            program,
            args,
            cwd,
            cols,
            rows,
        } => {
            let domain = LocalDomain::new();
            let handle = domain
                .spawn_pane(&PaneConfig::Local {
                    command: program.clone(),
                    args: args.clone(),
                    cwd: cwd.clone(),
                    cols: *cols,
                    rows: *rows,
                })
                .await?;
            Ok(match window {
                Some(window) => app.register_pane_in_window(handle, *cols, *rows, window),
                None => app.register_pane_in_group(handle, *cols, *rows, group),
            })
        }
        ConversationSpec::Ssh {
            target,
            identity,
            password: url_password,
            cols,
            rows,
        } => {
            let (user, host, port) = parse_ssh_target(target)?;
            let domain = RemoteSshDomain::new();
            let handle = domain
                .spawn_pane(&PaneConfig::Ssh {
                    host,
                    port,
                    user,
                    identity: identity.clone(),
                    password: url_password.clone().or(password),
                    cols: *cols,
                    rows: *rows,
                })
                .await?;
            Ok(match window {
                Some(window) => app.register_pane_in_window(handle, *cols, *rows, window),
                None => app.register_pane_in_group(handle, *cols, *rows, group),
            })
        }
        ConversationSpec::Wsl { args, cols, rows } => {
            let domain = LocalDomain::new();
            let handle = domain
                .spawn_pane(&PaneConfig::Local {
                    command: "wsl.exe".to_string(),
                    args: args.clone(),
                    cwd: None,
                    cols: *cols,
                    rows: *rows,
                })
                .await?;
            Ok(match window {
                Some(window) => app.register_pane_in_window(handle, *cols, *rows, window),
                None => app.register_pane_in_group(handle, *cols, *rows, group),
            })
        }
    }
}
