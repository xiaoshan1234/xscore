//! tml add / tml del：统一的 shell/ssh/wsl 对话与 tmux window/pane 管理。

use anyhow::{anyhow, Context, Result};

use xscore::domain::{Domain, LocalDomain, RemoteSshDomain, TerminalConfig, TerminalId};
use xscore::platform::tmux::quote_arg;
use xscore::SshOptions;

use crate::cli::{
    parse_conversation_url, parse_environment_assignment, parse_ssh_target, ConversationSpec,
    TmlAddRequest,
};

use crate::app::App;
use crate::app::{GroupId, GroupKind, WindowId, WindowKind};
use crate::layout::Direction;
use crate::render::Output;
use crate::tmux::session::reconcile_session;

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
        if !args.env.is_empty() {
            return Err(anyhow!(
                "--env is only supported when creating shell, SSH, or WSL conversations"
            ));
        }
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
    let environment = merged_environment(&spec, &args.env)?;
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
    let id = spawn(
        app,
        &spec,
        &environment,
        args.password,
        group,
        target_window,
    )
    .await?;
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
    let id = TerminalId::new(raw.parse().context("pane id")?);
    app.tmux
        .get(&id)
        .map(|meta| meta.handle.clone())
        .ok_or_else(|| anyhow!("tmux pane not found: {id}"))
}

fn split_target(args: &TmlAddRequest) -> Result<Option<(Direction, TerminalId)>> {
    if let Some(raw) = &args.sph {
        return Ok(Some((
            Direction::Horizontal,
            TerminalId::new(raw.parse().context("pane id")?),
        )));
    }
    if let Some(raw) = &args.spv {
        return Ok(Some((
            Direction::Vertical,
            TerminalId::new(raw.parse().context("pane id")?),
        )));
    }
    Ok(None)
}

async fn spawn(
    app: &mut App,
    spec: &ConversationSpec,
    environment: &[(String, String)],
    password: Option<String>,
    group: GroupId,
    window: Option<WindowId>,
) -> Result<TerminalId> {
    match spec {
        ConversationSpec::Shell {
            program,
            args,
            cwd,
            env: _,
            cols,
            rows,
        } => {
            let domain = LocalDomain::new();
            let handle = domain
                .spawn_terminal(&TerminalConfig::Local {
                    command: program.clone(),
                    args: args.clone(),
                    cwd: cwd.clone(),
                    env: environment.to_vec(),
                    cols: *cols,
                    rows: *rows,
                })
                .await?;
            Ok(match window {
                Some(window) => app.register_terminal_in_window(handle, *cols, *rows, window),
                None => app.register_terminal_in_group(handle, *cols, *rows, group),
            })
        }
        ConversationSpec::Ssh {
            target,
            identity,
            password: url_password,
            env: _,
            cols,
            rows,
        } => {
            let (user, host, port) = parse_ssh_target(target)?;
            let domain = RemoteSshDomain::new();
            let handle = domain
                .spawn_terminal(&TerminalConfig::Ssh {
                    host,
                    port,
                    user,
                    identity: identity.clone(),
                    password: url_password.clone().or(password),
                    env: environment.to_vec(),
                    cols: *cols,
                    rows: *rows,
                    options: SshOptions::default(),
                })
                .await?;
            Ok(match window {
                Some(window) => app.register_terminal_in_window(handle, *cols, *rows, window),
                None => app.register_terminal_in_group(handle, *cols, *rows, group),
            })
        }
        ConversationSpec::Wsl {
            args,
            env: _,
            cols,
            rows,
        } => {
            let domain = LocalDomain::new();
            let wsl_env = wsl_environment(environment);
            let handle = domain
                .spawn_terminal(&TerminalConfig::Local {
                    command: "wsl.exe".to_string(),
                    args: args.clone(),
                    cwd: None,
                    env: wsl_env,
                    cols: *cols,
                    rows: *rows,
                })
                .await?;
            Ok(match window {
                Some(window) => app.register_terminal_in_window(handle, *cols, *rows, window),
                None => app.register_terminal_in_group(handle, *cols, *rows, group),
            })
        }
    }
}

fn merged_environment(spec: &ConversationSpec, extra: &[String]) -> Result<Vec<(String, String)>> {
    let mut environment = Vec::new();
    for (name, value) in spec.environment() {
        upsert_environment(&mut environment, name.clone(), value.clone());
    }
    for assignment in extra {
        let (name, value) = parse_environment_assignment(assignment)?;
        upsert_environment(&mut environment, name, value);
    }
    Ok(environment)
}

fn upsert_environment(environment: &mut Vec<(String, String)>, name: String, value: String) {
    if let Some((_, existing)) = environment
        .iter_mut()
        .find(|(existing, _)| existing == &name)
    {
        *existing = value;
    } else {
        environment.push((name, value));
    }
}

fn wsl_environment(environment: &[(String, String)]) -> Vec<(String, String)> {
    let inherited = std::env::var("WSLENV").ok();
    merge_wslenv(environment, inherited.as_deref())
}

fn merge_wslenv(
    environment: &[(String, String)],
    inherited: Option<&str>,
) -> Vec<(String, String)> {
    let mut merged = environment.to_vec();
    if environment.is_empty() {
        return merged;
    }

    let explicit = environment
        .iter()
        .rev()
        .find(|(name, _)| name.eq_ignore_ascii_case("WSLENV"))
        .map(|(_, value)| value.as_str());
    let mut entries: Vec<String> = explicit
        .or(inherited)
        .unwrap_or_default()
        .split(':')
        .filter(|entry| !entry.is_empty())
        .map(str::to_string)
        .collect();

    for (name, _) in environment {
        if name.eq_ignore_ascii_case("WSLENV") {
            continue;
        }
        if !entries
            .iter()
            .any(|entry| entry.split('/').next() == Some(name.as_str()))
        {
            entries.push(name.clone());
        }
    }
    upsert_environment(&mut merged, "WSLENV".to_string(), entries.join(":"));
    merged
}

#[cfg(test)]
mod tests {
    use super::{merge_wslenv, merged_environment, upsert_environment};
    use crate::cli::parse_conversation_url;

    #[test]
    fn environment_merging_keeps_last_value() {
        let mut environment = vec![("FOO".into(), "old".into())];
        upsert_environment(&mut environment, "FOO".into(), "new".into());
        assert_eq!(environment, [("FOO".into(), "new".into())]);

        let spec = parse_conversation_url("shell://bash?env=FOO%3Durl&env=BAR%3Done").unwrap();
        let merged = merged_environment(&spec, &["FOO=cli".into()]).unwrap();
        assert_eq!(
            merged,
            [("FOO".into(), "cli".into()), ("BAR".into(), "one".into())]
        );
    }

    #[test]
    fn wsl_environment_propagates_explicit_and_inherited_names() {
        let environment = vec![("FOO".into(), "bar".into())];
        assert_eq!(
            merge_wslenv(&environment, Some("EXISTING/p:FOO")),
            [
                ("FOO".into(), "bar".into()),
                ("WSLENV".into(), "EXISTING/p:FOO".into())
            ]
        );

        assert_eq!(
            merge_wslenv(&environment, None),
            [
                ("FOO".into(), "bar".into()),
                ("WSLENV".into(), "FOO".into())
            ]
        );
    }
}
