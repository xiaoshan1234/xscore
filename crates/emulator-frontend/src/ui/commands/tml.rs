//! tml create：统一的 shell/ssh/wsl 对话创建。

use anyhow::{anyhow, Context, Result};

use emulator::domain::{Domain, LocalDomain, PaneConfig, PaneId, RemoteSshDomain};

use crate::cli::{parse_conversation_url, parse_ssh_target, ConversationSpec, TmlCreateArgs};
use crate::ui::app::App;
use crate::ui::layout::Direction;
use crate::ui::render::Output;

pub async fn create(app: &mut App, args: TmlCreateArgs) -> Result<Output> {
    let spec = parse_conversation_url(&args.url)?;
    let split = split_target(&args)?;
    if let Some((_, target)) = &split {
        if !app.layout.contains(*target) {
            return Err(anyhow!("pane not found in layout: {target}"));
        }
    }
    let id = spawn(app, &spec, args.password).await?;
    match split {
        Some((direction, target)) => app.layout.split_pane(direction, target, id),
        // 未指定 --sph/--spv 时，默认创建新窗口。
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

fn split_target(args: &TmlCreateArgs) -> Result<Option<(Direction, PaneId)>> {
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

async fn spawn(app: &mut App, spec: &ConversationSpec, password: Option<String>) -> Result<PaneId> {
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
            Ok(app.register_pane(handle, *cols, *rows))
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
            Ok(app.register_pane(handle, *cols, *rows))
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
            Ok(app.register_pane(handle, *cols, *rows))
        }
    }
}
