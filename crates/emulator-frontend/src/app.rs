//! 前端应用状态：Domain/pane 注册表、布局树与命令处理。

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use tokio::sync::Mutex;

use emulator::domain::{
    enumerate, make_pane, Domain, LocalDomain, PaneConfig, PaneHandle, PaneId, PaneKind,
    RemoteSshDomain, TmuxDomain, TmuxPane, TmuxPaneInfo,
};
use emulator::platform::tmux::{TmuxControl, TmuxEvent, TmuxHostConfig};
use emulator::platform::Size;
use emulator::term::{TerminalCore, VtTerm};

use crate::cli::{
    build_tmux_config, parse_ssh_target, Cmd, PaneCmd, SplitDirection, TmuxArgs, WindowCmd,
};
use crate::layout::{Direction, Layout, PaneRect, TmuxContainer};
use crate::output::Output;
use crate::tmux_events::{classify_tmux_event, diff_tmux_structure, StructureAction};

struct PaneEntry {
    handle: Arc<dyn PaneHandle>,
    term: Arc<Mutex<VtTerm>>,
}

struct TmuxMeta {
    session: String,
    window: String,
    handle: String,
    window_active: bool,
    rect: PaneRect,
    window_cols: u16,
    window_rows: u16,
}

pub struct App {
    panes: HashMap<PaneId, PaneEntry>,
    layout: Layout,
    tmux: HashMap<PaneId, TmuxMeta>,
}

impl App {
    pub fn new() -> Self {
        Self {
            panes: HashMap::new(),
            layout: Layout::new(),
            tmux: HashMap::new(),
        }
    }

    async fn shell(&mut self, program: String, args: Vec<String>) -> Result<Output> {
        let id = self.spawn_local(program, args).await?;
        self.layout.add(id);
        let mut out = Output::new();
        out.line(format!("pane: {id}  kind: local  80x24"));
        Ok(out)
    }

    async fn split(
        &mut self,
        direction: SplitDirection,
        program: String,
        args: Vec<String>,
    ) -> Result<Output> {
        let direction = match direction {
            SplitDirection::Horizontal => Direction::Horizontal,
            SplitDirection::Vertical => Direction::Vertical,
        };
        let id = self.spawn_local(program, args).await?;
        self.layout.split_focused(direction, id);
        let mut out = Output::new();
        out.line(format!("pane: {id}  kind: local  80x24"));
        Ok(out)
    }

    async fn spawn_local(&mut self, command: String, args: Vec<String>) -> Result<PaneId> {
        let cols = 80u16;
        let rows = 24u16;
        let domain = LocalDomain::new();
        let handle = domain
            .spawn_pane(&PaneConfig::Local {
                command,
                args,
                cwd: None,
                cols,
                rows,
            })
            .await?;
        Ok(self.register_pane(handle, cols, rows))
    }

    async fn tmux(&mut self, tmux_args: &TmuxArgs, app: Arc<Mutex<App>>) -> Result<Output> {
        let session = tmux_args.session.clone();
        let (socket, mut host, want_password) = build_tmux_config(tmux_args)?;
        if want_password {
            let password = tmux_args
                .password_value
                .clone()
                .ok_or_else(|| anyhow!("password required for --ssh"))?;
            let askpass = std::env::current_exe()?.to_string_lossy().into_owned();
            if let TmuxHostConfig::Ssh {
                password: slot,
                askpass: ap,
                ..
            } = &mut host
            {
                *slot = Some(password);
                *ap = Some(askpass);
            }
        }
        let domain = TmuxDomain::new();
        let panes = domain.attach(&host, &socket, &session).await?;
        let control = panes.first().map(|pane| pane.control().clone());

        let mut out = Output::new();
        let mut groups: Vec<(String, u16, u16, bool, Vec<PaneRect>)> = Vec::new();
        let mut index: HashMap<String, usize> = HashMap::new();
        for pane in panes {
            let (cols, rows) = (pane.cols(), pane.rows());
            let (x, y) = (pane.x(), pane.y());
            let window = pane.window().to_string();
            let (window_cols, window_rows) = (pane.window_cols(), pane.window_rows());
            let window_active = pane.window_active();
            let handle = pane.tmux_handle().to_string();
            let id = self.register_pane(pane, cols, rows);
            let rect = PaneRect {
                id,
                x,
                y,
                cols,
                rows,
            };
            self.tmux.insert(
                id,
                TmuxMeta {
                    session: session.clone(),
                    window: window.clone(),
                    handle,
                    window_active,
                    rect,
                    window_cols,
                    window_rows,
                },
            );
            let group = *index.entry(window.clone()).or_insert_with(|| {
                groups.push((window, window_cols, window_rows, window_active, Vec::new()));
                groups.len() - 1
            });
            groups[group].4.push(rect);
        }
        for (window, window_cols, window_rows, window_active, pane_rects) in groups {
            if window_active {
                let active = pane_rects.first().map(|rect| rect.id);
                let container = TmuxContainer {
                    window: window.clone(),
                    cols: window_cols,
                    rows: window_rows,
                    panes: pane_rects.clone(),
                    active,
                };
                self.layout.add_mirror(container);
                out.line(format!(
                    "tmux window {window} (active): {} pane(s)",
                    pane_rects.len()
                ));
            } else {
                out.line(format!(
                    "tmux window {window}: {} pane(s)",
                    pane_rects.len()
                ));
            }
        }

        if let Some(control) = control {
            let mut events = control.subscribe();
            let event_app = app.clone();
            let event_session = session.clone();
            tokio::spawn(async move {
                while let Some(event) = events.recv().await {
                    let should_stop =
                        handle_tmux_event(&event_app, &event_session, &control, event).await;
                    if should_stop {
                        break;
                    }
                }
            });
        }
        Ok(out)
    }

    async fn ssh(
        &mut self,
        target: String,
        identity: Option<String>,
        password: Option<String>,
    ) -> Result<Output> {
        let (user, host, port) = parse_ssh_target(&target)?;
        let cols = 80u16;
        let rows = 24u16;
        let domain = RemoteSshDomain::new();
        let handle = domain
            .spawn_pane(&PaneConfig::Ssh {
                host,
                port,
                user,
                identity,
                password,
                cols,
                rows,
            })
            .await?;
        let id = self.attach_pane(handle, cols, rows);
        let mut out = Output::new();
        out.line(format!("pane: {id}  kind: ssh  {cols}x{rows}"));
        Ok(out)
    }

    async fn wsl(&mut self, args: Vec<String>) -> Result<Output> {
        let id = self.spawn_local("wsl.exe".to_string(), args).await?;
        self.layout.add(id);
        let mut out = Output::new();
        out.line(format!("pane: {id}  kind: wsl  80x24"));
        Ok(out)
    }

    fn register_pane(&mut self, handle: Arc<dyn PaneHandle>, cols: u16, rows: u16) -> PaneId {
        let id = handle.id();
        let term = Arc::new(Mutex::new(VtTerm::new(cols, rows)));
        let mut rx = handle.subscribe();
        let pump = term.clone();
        tokio::spawn(async move {
            while let Ok(chunk) = rx.recv().await {
                pump.lock().await.feed(&chunk);
            }
        });
        self.panes.insert(id, PaneEntry { handle, term });
        id
    }

    fn attach_pane(&mut self, handle: Arc<dyn PaneHandle>, cols: u16, rows: u16) -> PaneId {
        let id = self.register_pane(handle, cols, rows);
        self.layout.add(id);
        id
    }

    async fn write(&self, pane: String, text: Vec<String>) -> Result<Output> {
        let entry = self.get_pane(&pane)?;
        let mut data = text.join(" ").into_bytes();
        let eol: &[u8] = if entry.handle.kind() == PaneKind::Local && cfg!(windows) {
            b"\r\n"
        } else {
            b"\n"
        };
        data.extend_from_slice(eol);
        entry.handle.send_input(&data).await?;
        Ok(Output::new())
    }

    async fn read(&self, pane: String) -> Result<Output> {
        let entry = self.get_pane(&pane)?;
        // 给后台 pump 一个极短的静默窗口，让刚到达的字节先喂进网格。
        tokio::time::sleep(Duration::from_millis(60)).await;
        let snap = entry.term.lock().await.snapshot();
        let mut out = Output::new();
        out.push(&snap);
        if !snap.ends_with('\n') {
            out.push("\n");
        }
        Ok(out)
    }

    async fn resize(&self, pane: String, cols: u16, rows: u16) -> Result<Output> {
        let entry = self.get_pane(&pane)?;
        entry.handle.resize(Size::new(cols, rows)).await?;
        entry.term.lock().await.resize(cols, rows);
        Ok(Output::new())
    }

    async fn list(&self) -> Output {
        let mut out = Output::new();
        out.line(format!(
            "{:<10} {:<8} {:<6} {:<6}",
            "PANE", "KIND", "COLS", "ROWS"
        ));
        let mut ids: Vec<PaneId> = self.panes.keys().copied().collect();
        ids.sort_by_key(|p| p.as_u64());
        for id in ids {
            let entry = &self.panes[&id];
            let (cols, rows) = {
                let term = entry.term.lock().await;
                (term.cols(), term.rows())
            };
            let kind = match entry.handle.kind() {
                PaneKind::Local => "local",
                PaneKind::Ssh => "ssh",
                PaneKind::Tmux => "tmux",
            };
            out.line(format!("{:<10} {:<8} {:<6} {:<6}", id, kind, cols, rows));
        }
        out
    }

    fn focus(&mut self, pane: String) -> Result<Output> {
        let id = PaneId::new(pane.parse().context("pane id")?);
        if !self.panes.contains_key(&id) {
            return Err(anyhow!("pane not found: {id}"));
        }
        if let Some(window) = self.tmux.get(&id).map(|m| m.window.clone()) {
            if !self.layout.set_mirror_active(&window, id) {
                return Err(anyhow!("tmux window not found in layout: {window}"));
            }
        } else {
            self.layout.focus(id);
        }
        let mut out = Output::new();
        out.line(format!("focused: {id}"));
        Ok(out)
    }

    async fn pane_del(&mut self, pane: String) -> Result<Output> {
        let id = PaneId::new(pane.parse().context("pane id")?);
        if !self.panes.contains_key(&id) {
            return Err(anyhow!("pane not found: {id}"));
        }
        {
            let entry = self.panes.get(&id).unwrap();
            entry.handle.close().await?;
        }
        self.remove_pane_local(id);
        let mut out = Output::new();
        out.line(format!("deleted pane: {id}"));
        Ok(out)
    }

    async fn pane_fetch(&self, pane: String, line: Option<usize>) -> Result<Output> {
        let id = PaneId::new(pane.parse().context("pane id")?);
        let start = match line {
            Some(line_count) => format!("-{line_count}"),
            None => "-".to_string(),
        };
        let entry = self
            .panes
            .get(&id)
            .ok_or_else(|| anyhow!("pane not found: {id}"))?;
        let tmux_pane = entry
            .handle
            .as_any()
            .downcast_ref::<TmuxPane>()
            .ok_or_else(|| anyhow!("pane {id} is not a tmux pane"))?;
        let handle = tmux_pane.tmux_handle();
        let lines = tmux_pane
            .send_command(&format!("capture-pane -p -S {start} -t {handle}"))
            .await?;
        let mut out = Output::new();
        for line in &lines {
            out.line(line.trim_end());
        }
        Ok(out)
    }

    async fn window_del(&mut self, window: String) -> Result<Output> {
        let pane_ids: Vec<PaneId> = self
            .tmux
            .iter()
            .filter(|(_, m)| m.window == window)
            .map(|(id, _)| *id)
            .collect();
        if pane_ids.is_empty() {
            return Err(anyhow!("tmux window not found: {window}"));
        }
        {
            let entry = self.panes.get(&pane_ids[0]).unwrap();
            if let Some(tmux_pane) = entry.handle.as_any().downcast_ref::<TmuxPane>() {
                tmux_pane
                    .send_command(&format!("kill-window -t {window}"))
                    .await?;
            }
        }
        for id in pane_ids {
            self.panes.remove(&id);
            self.tmux.remove(&id);
        }
        self.layout.remove_window(&window);
        let mut out = Output::new();
        out.line(format!("deleted window: {window}"));
        Ok(out)
    }

    async fn window_select(&mut self, handle: String) -> Result<Output> {
        let Some(session) = self
            .tmux
            .values()
            .find(|meta| meta.window == handle)
            .map(|meta| meta.session.clone())
        else {
            return Err(anyhow!("tmux window not found: {handle}"));
        };
        let control = self
            .tmux_control_for(&session)
            .ok_or_else(|| anyhow!("tmux session control unavailable"))?;
        control.send(&format!("select-window -t {handle}")).await?;
        let _ = self.reconcile_session(&session, &control).await?;
        let mut out = Output::new();
        out.line(format!("selected window: {handle}"));
        Ok(out)
    }

    fn window_list(&self) -> Output {
        let mut windows: HashMap<String, (bool, usize)> = HashMap::new();
        for meta in self.tmux.values() {
            let entry = windows
                .entry(meta.window.clone())
                .or_insert((meta.window_active, 0));
            entry.1 += 1;
        }
        let mut rows: Vec<_> = windows.into_iter().collect();
        rows.sort_by(|a, b| a.0.cmp(&b.0));
        let mut out = Output::new();
        out.line(format!("{:<10} {:<8} {:<8}", "WINDOW", "ACTIVE", "PANES"));
        for (handle, (active, pane_count)) in rows {
            out.line(format!(
                "{:<10} {:<8} {:<8}",
                handle,
                if active { "yes" } else { "no" },
                pane_count
            ));
        }
        out
    }

    async fn sync(&mut self) -> Result<Output> {
        let sessions: Vec<String> = {
            let set: HashSet<String> = self.tmux.values().map(|m| m.session.clone()).collect();
            set.into_iter().collect()
        };
        let mut removed_total = 0usize;
        let mut added_total = 0usize;
        for session in sessions {
            let Some(control) = self.tmux_control_for(&session) else {
                continue;
            };
            let (removed, added) = self.reconcile_session(&session, &control).await?;
            removed_total += removed;
            added_total += added;
        }
        let mut out = Output::new();
        out.line(format!(
            "sync: removed {removed_total} pane(s), added {added_total} pane(s)"
        ));
        Ok(out)
    }

    pub fn status(&self) -> Output {
        let mut out = Output::new();
        out.line(format!("daemon: {} pane(s)", self.panes.len()));
        let mut sessions: Vec<String> = self
            .tmux
            .values()
            .map(|m| m.session.clone())
            .collect::<HashSet<_>>()
            .into_iter()
            .collect();
        sessions.sort();
        for session in sessions {
            out.line(format!("tmux session: {session}"));
        }
        out
    }

    fn tmux_control_for(&self, session: &str) -> Option<Arc<TmuxControl>> {
        let rep_id = self
            .tmux
            .iter()
            .find(|(_, m)| m.session == session)
            .map(|(id, _)| *id)?;
        let entry = self.panes.get(&rep_id)?;
        entry
            .handle
            .as_any()
            .downcast_ref::<TmuxPane>()
            .map(|pane| pane.control().clone())
    }

    async fn reconcile_session(
        &mut self,
        session: &str,
        control: &Arc<TmuxControl>,
    ) -> Result<(usize, usize)> {
        let windows = enumerate(control, session).await?;
        let local: HashMap<String, PaneId> = self
            .tmux
            .iter()
            .filter(|(_, m)| m.session == session)
            .map(|(id, m)| (m.handle.clone(), *id))
            .collect();
        let (gone, new_panes) = diff_tmux_structure(&local, &windows);

        for id in &gone {
            self.remove_pane_local(*id);
        }
        for spec in &new_panes {
            let pane_info = TmuxPaneInfo {
                handle: spec.handle.clone(),
                cols: spec.cols,
                rows: spec.rows,
                x: spec.x,
                y: spec.y,
            };
            let new_pane = make_pane(
                control.clone(),
                spec.window.clone(),
                &pane_info,
                spec.window_cols,
                spec.window_rows,
                spec.window_active,
            );
            let id = self.register_pane(new_pane, spec.cols, spec.rows);
            self.tmux.insert(
                id,
                TmuxMeta {
                    session: session.to_string(),
                    window: spec.window.clone(),
                    handle: spec.handle.clone(),
                    window_active: spec.window_active,
                    rect: PaneRect {
                        id,
                        x: spec.x,
                        y: spec.y,
                        cols: spec.cols,
                        rows: spec.rows,
                    },
                    window_cols: spec.window_cols,
                    window_rows: spec.window_rows,
                },
            );
        }

        // 刷新已存在 pane 的几何与 active 标志（当前 window 可能切换）。
        for window in &windows {
            for pane in &window.panes {
                if let Some(id) = local.get(&pane.handle) {
                    if let Some(meta) = self.tmux.get_mut(id) {
                        meta.window = window.handle.clone();
                        meta.window_active = window.active;
                        meta.rect = PaneRect {
                            id: *id,
                            x: pane.x,
                            y: pane.y,
                            cols: pane.cols,
                            rows: pane.rows,
                        };
                        meta.window_cols = window.cols;
                        meta.window_rows = window.rows;
                    }
                }
            }
        }

        self.rebuild_active_mirror(session);

        Ok((gone.len(), new_panes.len()))
    }

    fn rebuild_active_mirror(&mut self, session: &str) {
        let windows: HashSet<String> = self
            .tmux
            .values()
            .filter(|meta| meta.session == session)
            .map(|meta| meta.window.clone())
            .collect();
        for window in &windows {
            self.layout.remove_window(window);
        }

        let Some(active_meta) = self
            .tmux
            .values()
            .find(|meta| meta.session == session && meta.window_active)
        else {
            return;
        };

        let mut rects: Vec<PaneRect> = self
            .tmux
            .values()
            .filter(|meta| meta.session == session && meta.window == active_meta.window)
            .map(|meta| meta.rect)
            .collect();
        rects.sort_by_key(|rect| (rect.y, rect.x));

        let active = rects.first().map(|rect| rect.id);
        let container = TmuxContainer {
            window: active_meta.window.clone(),
            cols: active_meta.window_cols,
            rows: active_meta.window_rows,
            panes: rects,
            active,
        };
        self.layout.add_mirror(container);
    }

    fn remove_pane_local(&mut self, id: PaneId) {
        self.panes.remove(&id);
        self.tmux.remove(&id);
        self.layout.remove_pane(id);
    }

    fn layout_cmd(&self) -> Output {
        let mut out = Output::new();
        out.line(self.layout.render());
        if let Some(focused) = self.layout.focused() {
            out.line(format!("focused: {focused}"));
        }
        out
    }

    fn get_pane(&self, s: &str) -> Result<&PaneEntry> {
        let id = PaneId::new(s.parse().context("pane id")?);
        self.panes
            .get(&id)
            .ok_or_else(|| anyhow!("pane not found: {id}"))
    }
}

async fn handle_tmux_event(
    app: &Arc<Mutex<App>>,
    session: &str,
    control: &Arc<TmuxControl>,
    event: TmuxEvent,
) -> bool {
    match classify_tmux_event(&event) {
        StructureAction::RemoveWindow(window) => {
            let mut guard = app.lock().await;
            let pane_ids: Vec<PaneId> = guard
                .tmux
                .iter()
                .filter(|(_, meta)| meta.session == session && meta.window == window)
                .map(|(id, _)| *id)
                .collect();
            for id in pane_ids {
                guard.remove_pane_local(id);
            }
            false
        }
        StructureAction::Shutdown => {
            let mut guard = app.lock().await;
            let pane_ids: Vec<PaneId> = guard
                .tmux
                .iter()
                .filter(|(_, meta)| meta.session == session)
                .map(|(id, _)| *id)
                .collect();
            for id in pane_ids {
                guard.remove_pane_local(id);
            }
            true
        }
        StructureAction::ReconcileSession => {
            let mut guard = app.lock().await;
            if let Err(error) = guard.reconcile_session(session, control).await {
                tracing::warn!(session, error = %error, "tmux event reconcile failed");
            }
            false
        }
    }
}

pub fn help() -> Output {
    let mut out = Output::new();
    out.line("commands:");
    out.line("  shell <program> [args...]   spawn a local shell pane");
    out.line("  ssh <user@host[:port]> [identity]   spawn a remote ssh pane");
    out.line("  wsl [args...]               spawn a WSL shell pane (Windows)");
    out.line("  tmux <session> [-L <socket>] [--ssh <dest> | --wsl] [--password]   attach to a tmux session");
    out.line("  split h|v <program> [args...]   split the focused pane");
    out.line("  write <pane-id> <text>      send text + newline to the pane");
    out.line("  read  <pane-id>             print accumulated output");
    out.line("  resize <pane-id> <c> <r>    resize the pane");
    out.line("  focus <pane-id>             move focus");
    out.line("  layout                      print the pane tree");
    out.line("  pane del <pane-id>          delete a pane");
    out.line("  pane fetch <pane-id> [--line <N>]   fetch tmux pane scrollback (all, or last N)");
    out.line("  window del <handle>         delete a tmux window");
    out.line("  window list                 list tmux windows");
    out.line("  window select <handle>      switch to another tmux window");
    out.line("  sync                        sync local panes with the tmux structure");
    out.line("  list                        list panes");
    out.line("  daemon serve|status|stop    run / inspect / stop the resident daemon");
    out
}

/// 分发一条命令，返回输出文本。
pub async fn dispatch(app: &Arc<Mutex<App>>, cmd: Cmd) -> Result<Output> {
    let mut guard = app.lock().await;
    let out = match cmd {
        Cmd::Shell { program, args } => guard.shell(program, args).await?,
        Cmd::Ssh {
            target,
            identity,
            password,
        } => guard.ssh(target, identity, password).await?,
        Cmd::Wsl { args } => guard.wsl(args).await?,
        Cmd::Tmux(tmux_args) => guard.tmux(&tmux_args, app.clone()).await?,
        Cmd::Split {
            direction,
            program,
            args,
        } => guard.split(direction, program, args).await?,
        Cmd::Write { pane, text } => guard.write(pane, text).await?,
        Cmd::Read { pane } => guard.read(pane).await?,
        Cmd::Resize { pane, cols, rows } => guard.resize(pane, cols, rows).await?,
        Cmd::Focus { pane } => guard.focus(pane)?,
        Cmd::Layout => guard.layout_cmd(),
        Cmd::Pane(PaneCmd::Del { pane }) => guard.pane_del(pane).await?,
        Cmd::Pane(PaneCmd::Fetch { pane, line }) => guard.pane_fetch(pane, line).await?,
        Cmd::Window(WindowCmd::Del { window }) => guard.window_del(window).await?,
        Cmd::Window(WindowCmd::Select { window }) => guard.window_select(window).await?,
        Cmd::Window(WindowCmd::List) => guard.window_list(),
        Cmd::Sync => guard.sync().await?,
        Cmd::List => guard.list().await,
        Cmd::Help => help(),
        Cmd::Daemon(_) => return Err(anyhow!("daemon subcommand is handled by the entry point")),
    };
    Ok(out)
}
