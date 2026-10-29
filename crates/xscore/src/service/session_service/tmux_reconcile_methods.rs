use super::*;
use crate::infra::tmux::{
    PaneLine, TmuxControl, TmuxEvent, TmuxHostConfig, TmuxSocket, WindowLine,
};
use crate::service::backend::BackendEvent;
use crate::service::buffer_service::{BufferId, BufferOrigin};
use crate::service::session_service::service::TmuxView;
use crate::service::session_service::SessionId;
use crate::ui::window::Window;
use crate::ui::{PaneId, PaneInfo, UiEvent, WindowBackend, WindowId, WindowInfo};
use crate::{Error, Result};
use std::collections::HashSet;
use std::sync::Arc;

impl SessionService {
    /// Re-read every window and pane of the session from one enumeration.
    ///
    /// Idempotent: rows are matched by handle (`@N`/`%N`, never the index), so
    /// running it twice changes nothing. Rows tmux no longer has are dropped.
    /// `local` is the in-process client (daemon mode passes `None`: the daemon
    /// already owns the subscribers, and rows only mirror them).
    pub(super) async fn reconcile_all(
        &self,
        id: SessionId,
        view: &TmuxView,
        local: Option<&Arc<TmuxControl>>,
    ) -> Result<bool> {
        let _guard = self.tmux.reconcile_guard().await;

        let mut changed = false;
        let mut seen: HashSet<String> = HashSet::new();
        for window in &view.windows {
            seen.insert(window.handle.clone());
            changed |= self.sync_window(id, view, window, local).await?;
        }

        let stale: Vec<WindowId> = self
            .ui
            .windows()
            .await
            .iter()
            .filter(|w| w.info.session_id == id)
            .filter(|w| match w.info.backend.tmux_handle() {
                Some(handle) => !seen.contains(handle),
                None => false,
            })
            .map(|w| w.info.id)
            .collect();
        for wid in stale {
            self.forget_window_row(wid).await;
            changed = true;
        }

        if changed {
            self.rebuild_backend(id).await?;
            self.save_state().await?;
        }
        Ok(changed)
    }

    /// Re-read one window (an event's answer to "what changed?").
    pub(super) async fn reconcile_window(
        &self,
        id: SessionId,
        view: &TmuxView,
        handle: &str,
        local: Option<&Arc<TmuxControl>>,
    ) -> Result<bool> {
        let _guard = self.tmux.reconcile_guard().await;

        let Some(window) = view.windows.iter().find(|w| w.handle == handle) else {
            // Gone between the notification and the query: treat it as the
            // close it already is.
            let changed = self.forget_window_by_handle(id, handle).await;
            if changed {
                self.rebuild_backend(id).await?;
                self.save_state().await?;
            }
            return Ok(changed);
        };

        let changed = self.sync_window(id, view, window, local).await?;
        if changed {
            self.rebuild_backend(id).await?;
            self.save_state().await?;
        }
        Ok(changed)
    }

    /// Enumerate through this process's client, then reconcile one window
    /// (the event pump's path — daemon mode has no event pump here).
    pub(super) async fn reconcile_window_from_control(
        &self,
        id: SessionId,
        control: &Arc<TmuxControl>,
        handle: &str,
    ) -> Result<bool> {
        let view = self.view_with(control, id).await?;
        self.reconcile_window(id, &view, handle, Some(control))
            .await
    }

    /// Upsert one window and its panes; start a subscriber for each pane that
    /// did not exist before.
    ///
    /// The caller holds TmuxRuntime's reconcile guard.
    pub(super) async fn sync_window(
        &self,
        id: SessionId,
        view: &TmuxView,
        window: &WindowLine,
        local: Option<&Arc<TmuxControl>>,
    ) -> Result<bool> {
        let Some(link_id) = self.control_link_id(id).await else {
            return Err(Error::Sink(format!(
                "session {id} is tmux-backed but has no control link"
            )));
        };
        let mut changed = false;

        let wid = match self.window_for_handle_in(id, &window.handle).await {
            Some(wid) => {
                let Some(arc) = self.ui.window(wid).await else {
                    return Ok(false);
                };
                let mut updated = arc.info.clone();
                let before = updated.clone();
                updated.name = window.name.clone();
                updated.cols = window.cols;
                updated.rows = window.rows;
                if updated != before {
                    let w = Window::new(updated);
                    *w.panes.lock().await = arc.panes.lock().await.clone();
                    self.ui.put_window(w).await;
                    changed = true;
                }
                wid
            }
            None => {
                let wid = WindowId::new();
                let info = WindowInfo {
                    id: wid,
                    session_id: id,
                    name: window.name.clone(),
                    created_at: chrono::Utc::now(),
                    cols: window.cols,
                    rows: window.rows,
                    backend: WindowBackend::Tmux {
                        handle: window.handle.clone(),
                    },
                };
                self.ui.put_window(Window::new(info)).await;
                changed = true;
                wid
            }
        };

        let empty: Vec<PaneLine> = Vec::new();
        let panes = view.panes.get(&window.handle).unwrap_or(&empty);

        let mut seen: HashSet<String> = HashSet::new();
        for pane in panes {
            seen.insert(pane.handle.clone());
            match self.pane_for_tmux_handle(id, &pane.handle).await {
                Some(existing) => {
                    let mut updated = existing.clone();
                    let before = updated.clone();
                    updated.cols = pane.cols;
                    updated.rows = pane.rows;
                    updated.x = pane.x;
                    updated.y = pane.y;
                    updated.window_id = Some(wid);
                    if updated != before {
                        self.ui.put_pane(updated).await;
                        changed = true;
                    }
                    if let Some(arc) = self.ui.window(wid).await {
                        let mut list = arc.panes.lock().await;
                        if !list.contains(&existing.id) {
                            list.push(existing.id);
                            changed = true;
                        }
                    }
                }
                None => {
                    let pid = PaneId::new();
                    let buffer_id = self
                        .buffer_service
                        .create(
                            id,
                            format!("tmux-{}", pane.handle),
                            BufferOrigin::Tmux {
                                link_id,
                                pane_handle: pane.handle.clone(),
                            },
                        )
                        .await;
                    let info = PaneInfo {
                        id: pid,
                        session_id: id,
                        name: format!("p-{pid}"),
                        created_at: chrono::Utc::now(),
                        cols: pane.cols,
                        rows: pane.rows,
                        x: pane.x,
                        y: pane.y,
                        buffer_id: Some(buffer_id),
                        window_id: Some(wid),
                    };
                    self.ui.put_pane(info).await;
                    if let Some(arc) = self.ui.window(wid).await {
                        arc.panes.lock().await.push(pid);
                    }
                    // A pane with no subscriber loses its bytes: tmux replays
                    // nothing, so the subscriber starts with the row.
                    self.start_pane_pump(id, local, buffer_id, &pane.handle)
                        .await;
                    changed = true;
                }
            }
        }

        // Panes this window no longer has.
        let mut stale = Vec::new();
        for pane in self.ui.panes().await {
            if pane.session_id != id || pane.window_id != Some(wid) {
                continue;
            }
            let Some(buffer_id) = pane.buffer_id else {
                continue;
            };
            let Some(buffer) = self.buffer_service.get(buffer_id).await else {
                continue;
            };
            if let BufferOrigin::Tmux { pane_handle, .. } = buffer.origin {
                if !seen.contains(&pane_handle) {
                    stale.push(pane.id);
                }
            }
        }
        for pid in stale {
            self.forget_pane_row(pid).await;
            changed = true;
        }

        Ok(changed)
    }

    pub(super) async fn window_for_handle_in(
        &self,
        id: SessionId,
        handle: &str,
    ) -> Option<WindowId> {
        self.ui
            .windows()
            .await
            .iter()
            .find(|w| w.info.session_id == id && w.info.backend.tmux_handle() == Some(handle))
            .map(|w| w.info.id)
    }

    pub(super) async fn pane_for_tmux_handle(&self, id: SessionId, key: &str) -> Option<PaneInfo> {
        for pane in self.ui.panes().await {
            if pane.session_id != id {
                continue;
            }
            let Some(buffer_id) = pane.buffer_id else {
                continue;
            };
            if let Some(buffer) = self.buffer_service.get(buffer_id).await {
                if matches!(
                    &buffer.origin,
                    BufferOrigin::Tmux { pane_handle, .. } if pane_handle == key
                ) {
                    return Some(pane);
                }
            }
        }
        None
    }

    /// Drop a window row and the panes that were displayed in it.
    pub(super) async fn forget_window_row(&self, wid: WindowId) {
        let pane_ids: Vec<PaneId> = match self.ui.window(wid).await {
            Some(window) => window.panes.lock().await.clone(),
            None => Vec::new(),
        };
        for pid in pane_ids {
            self.forget_pane_row(pid).await;
        }
        self.ui.remove_window(wid).await;
    }

    /// Drop a window by its tmux handle. Returns whether anything was removed.
    pub(super) async fn forget_window_by_handle(&self, id: SessionId, handle: &str) -> bool {
        match self.window_for_handle_in(id, handle).await {
            Some(wid) => {
                self.forget_window_row(wid).await;
                true
            }
            None => false,
        }
    }

    /// Drop a pane row, its runtime and its buffer. Used when tmux says the pane
    /// is gone, so `close()` only ever releases OUR pump (a control runtime's
    /// `close` is a no-op by design).
    pub(super) async fn forget_pane_row(&self, pid: PaneId) {
        let buffer_id = self.ui.pane(pid).await.and_then(|pane| pane.buffer_id);
        if let Some(buffer_id) = buffer_id {
            self.discard_buffer(buffer_id).await;
        }
        self.ui.remove_pane(pid).await;
    }

    // ----- pumps -----

    /// One task per session: structure changes become registry rows.
    ///
    /// In-process mode only. In daemon mode the client is the daemon's, so
    /// there is nothing here to subscribe: structure is enumerated on touch
    /// (arch.md §7) and `%exit` only flips the daemon slot's `alive` flag.
    pub(super) fn start_event_pump(
        &self,
        id: SessionId,
        control: Arc<TmuxControl>,
        mut events: tokio::sync::mpsc::UnboundedReceiver<TmuxEvent>,
    ) {
        let manager = self.clone();
        tokio::spawn(async move {
            // Ends when the channel closes — the reader thread drops every
            // sender on EOF, `close()` and `%exit`.
            while let Some(event) = events.recv().await {
                let action = manager
                    .backends
                    .lock()
                    .await
                    .get(&id)
                    .and_then(|backend| backend.accept_tmux_event(&event));
                match action {
                    // A rename and a layout change are the same question as an
                    // add: "what does this window look like now?"
                    Some(BackendEvent::Ui(UiEvent::WindowChanged {
                        tmux_handle: handle,
                    })) => {
                        if let Err(e) = manager
                            .reconcile_window_from_control(id, &control, &handle)
                            .await
                        {
                            tracing::warn!("tmux: could not reconcile window {handle}: {e}");
                        }
                    }
                    Some(BackendEvent::Ui(UiEvent::WindowRemoved {
                        tmux_handle: handle,
                    })) => {
                        if manager.forget_window_by_handle(id, &handle).await {
                            if let Err(e) = manager.rebuild_backend(id).await {
                                tracing::warn!("tmux: could not rebuild session manager: {e}");
                            }
                            if let Err(e) = manager.save_state().await {
                                tracing::warn!(
                                    "tmux: could not save state after {handle} closed: {e}"
                                );
                            }
                        }
                    }
                    // Nothing in xscore stores "the current window" yet, so
                    // these are informational (arch.md §7 lists them as state
                    // to update when a renderer exists).
                    Some(BackendEvent::Ignore) => {}
                    Some(BackendEvent::Ui(UiEvent::PaneChanged { .. }))
                    | Some(BackendEvent::Ui(UiEvent::PaneRemoved { .. })) => {
                        // Pane-level tmux notifications currently reconcile
                        // through their parent window.
                    }
                    Some(BackendEvent::Exit(reason)) => {
                        // The client is the session's single connection: with
                        // it gone the link is disconnected (which
                        // `link_is_connected` reports from the closed handle)
                        // and every pane pump ends, because `%exit` shuts the
                        // subscriber channels down.
                        tracing::info!("tmux: control client for session {id} exited: {reason}");
                        break;
                    }
                    None => {
                        tracing::warn!("tmux: session {id} has no backend SessionService");
                        break;
                    }
                }
            }
        });
    }

    /// Give a new pane its `%output %N` subscriber, wherever the client lives.
    ///
    /// In-process: a task here appending to `buffer_memory`. Daemon: one
    /// `Attach`, and the daemon's own pump fills the daemon's buffer — which is
    /// the whole point of moving the client there.
    pub(super) async fn start_pane_pump(
        &self,
        id: SessionId,
        local: Option<&Arc<TmuxControl>>,
        buffer_id: BufferId,
        key: &str,
    ) {
        if let Some(control) = local {
            self.start_output_pump(Arc::clone(control), buffer_id, key.to_string())
                .await;
            return;
        }
        if let Some(client) = &self.daemon {
            if let Err(e) = client.attach_control_buffer(id, buffer_id, key).await {
                // Not fatal: the row is still right, and the next data op
                // re-attaches (`control_runtime`).
                tracing::warn!("tmux: daemon could not subscribe pane {key}: {e}");
            }
        }
    }

    /// One task per pane: `%output %N` appended to that pane's buffer.
    pub(super) async fn start_output_pump(
        &self,
        control: Arc<TmuxControl>,
        buffer_id: BufferId,
        key: String,
    ) {
        let buffer = self.buffer_service.ring(buffer_id).await;
        let mut rx = match control.output(&key).await {
            Ok(rx) => rx,
            Err(e) => {
                tracing::warn!("tmux: could not subscribe to pane {key}: {e}");
                return;
            }
        };
        tokio::spawn(async move {
            while let Some(bytes) = rx.recv().await {
                buffer.lock().await.push_slice(&bytes);
            }
        });
    }

    /// Run one command on the session's control client, wherever it lives.
    ///
    /// Only the mutations that must stay on the one client use this
    /// (`new-window`, `split-window`, `kill-pane`). Data goes through the
    /// pane's runtime and structure through `Structure`; neither is an exec.
    pub(super) async fn tmux_send(&self, id: SessionId, command: &str) -> Result<Vec<String>> {
        match &self.daemon {
            Some(client) => client.tmux_command(id, command).await,
            None => self.ensure_local_control(id).await?.send(command).await,
        }
    }

    /// Re-read one window after a mutation and reconcile it into rows.
    pub(super) async fn refresh_window(&self, id: SessionId, handle: &str) -> Result<bool> {
        match &self.daemon {
            Some(_) => {
                let view = self.tmux_view(id).await?;
                self.reconcile_window(id, &view, handle, None).await
            }
            None => {
                let control = self.ensure_local_control(id).await?;
                self.reconcile_window_from_control(id, &control, handle)
                    .await
            }
        }
    }

    /// One enumeration through whichever client holds the session.
    pub(super) async fn tmux_view(&self, id: SessionId) -> Result<TmuxView> {
        match &self.daemon {
            Some(client) => {
                let structure = match client.structure(id).await {
                    Ok(structure) => structure,
                    // No client in the daemon yet (it restarted): opening one
                    // enumerates too.
                    Err(_) => {
                        let target = self.tmux_target(id).await?;
                        client
                            .open_tmux(
                                id,
                                target.host,
                                target.socket,
                                &target.name,
                                target.cols,
                                target.rows,
                            )
                            .await?
                    }
                };
                Ok(TmuxView::from_structure(structure))
            }
            None => {
                let control = self.ensure_local_control(id).await?;
                self.view_with(&control, id).await
            }
        }
    }

    // ----- one-shot tmux commands (arch.md §6, "two exec shapes") -----

    pub(super) async fn tmux_session_exists(
        &self,
        host: &TmuxHostConfig,
        socket: &TmuxSocket,
        name: &str,
    ) -> Result<bool> {
        let out = self
            .run_tmux(
                host,
                socket,
                &[
                    "has-session".to_string(),
                    "-t".to_string(),
                    name.to_string(),
                ],
            )
            .await?;
        if out.status.success() {
            return Ok(true);
        }
        if out.status.code() == Some(255) {
            return Err(self.tmux_failure(host, name, "has-session", &out));
        }
        // Exit 1 is "no such session": the answer, not a failure.
        Ok(false)
    }

    /// Run one `tmux` command and return its output (the other exec shape: no
    /// state to keep, so no client).
    pub(super) async fn run_tmux(
        &self,
        host: &TmuxHostConfig,
        socket: &TmuxSocket,
        args: &[String],
    ) -> Result<std::process::Output> {
        let argv = crate::infra::tmux::one_shot_argv(host, socket, args);
        let (program, rest) = argv
            .split_first()
            .ok_or_else(|| Error::Sink("empty tmux command line".into()))?;
        tokio::process::Command::new(program)
            .args(rest)
            .output()
            .await
            .map_err(|e| Error::TmuxUnreachable {
                host: crate::infra::tmux::host_label(host),
                reason: format!("spawn `{program}`: {e}"),
            })
    }

    /// Turn a failed one-shot into the error the failure semantics table names
    /// (arch.md §6): exit 255 means the host never answered, everything else
    /// means tmux answered and refused.
    pub(super) fn tmux_failure(
        &self,
        host: &TmuxHostConfig,
        session: &str,
        what: &str,
        out: &std::process::Output,
    ) -> Error {
        let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
        if out.status.code() == Some(255) {
            return Error::TmuxUnreachable {
                host: crate::infra::tmux::host_label(host),
                reason: if stderr.is_empty() {
                    "exit 255".into()
                } else {
                    stderr
                },
            };
        }
        Error::Sink(format!(
            "tmux {what} -t {session} failed ({}): {stderr}",
            out.status.code().unwrap_or(-1)
        ))
    }
}
