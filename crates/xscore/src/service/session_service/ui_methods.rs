use super::*;
use crate::service::buffer_service::{BufferId, BufferRuntime};
use crate::service::link_service::LinkId;
use crate::service::session_service::service::{pipe_transport, tmux_handle_from};
use crate::service::session_service::{SessionBackend, SessionId};
use crate::ui::window::Window;
use crate::ui::{PaneId, PaneInfo, WindowBackend, WindowId, WindowInfo};
use crate::{Error, Result};
use std::sync::Arc;

impl SessionService {
    pub async fn add_window(&self, name: Option<String>, cols: u16, rows: u16) -> Result<WindowId> {
        let sid = self.resolve_session(None).await?;
        self.add_window_in(sid, name, cols, rows).await
    }

    /// Create a window in `session`.
    ///
    /// A tmux-backed session gets a real window (`new-window` through its
    /// control client; tmux reports the `@N`, the name and the geometry). A
    /// native session gets a purely xscore-side window with the requested
    /// geometry — there is no renderer yet, so it displays nothing (arch.md
    /// §13).
    pub async fn add_window_in(
        &self,
        session: SessionId,
        name: Option<String>,
        cols: u16,
        rows: u16,
    ) -> Result<WindowId> {
        self.require_session(session).await?;
        match self.get_session(session).await?.backend {
            SessionBackend::Native => {
                let window_id = WindowId::new();
                let info = WindowInfo {
                    id: window_id,
                    session_id: session,
                    name: name.unwrap_or_else(|| format!("w-{}", &window_id.to_string()[..8])),
                    created_at: chrono::Utc::now(),
                    cols,
                    rows,
                    backend: WindowBackend::Native,
                };
                self.ui.put_window(Window::new(info)).await;
                self.save_state().await?;
                Ok(window_id)
            }
            SessionBackend::Tmux { .. } => {
                let lines = self
                    .tmux_send(
                        session,
                        &crate::infra::tmux::new_window_command(name.as_deref()),
                    )
                    .await?;
                let handle = tmux_handle_from(&lines, '@').ok_or_else(|| {
                    Error::Sink(format!(
                        "tmux new-window did not report a window handle: {lines:?}"
                    ))
                })?;
                // Read the new window back: the row must be tmux's view of it,
                // not our guess at its geometry or name.
                self.refresh_window(session, &handle).await?;
                self.window_for_handle_in(session, &handle)
                    .await
                    .ok_or_else(|| {
                        Error::Sink(format!(
                            "tmux created window {handle} but it did not become a row"
                        ))
                    })
            }
        }
    }

    /// Tear down a window row. Panes inside survive as orphans.
    ///
    /// The row is xscore's, not the backend's: for a tmux window the next
    /// reconciliation (any listing, or an event) brings it back, which is why
    /// the verb is about the row only.
    pub async fn del_window(&self, id: WindowId) -> Result<()> {
        self.ui.delete_window(id).await?;
        self.save_state().await?;
        Ok(())
    }

    // ===== add_pane / del_pane =====

    /// Create a pane in the resolved existing session. See `add_pane_in`.
    pub async fn add_pane(
        &self,
        name: Option<String>,
        cols: u16,
        rows: u16,
        x: u16,
        y: u16,
        window: Option<WindowId>,
    ) -> Result<PaneId> {
        let sid = self.resolve_session(None).await?;
        self.add_pane_in(sid, name, cols, rows, x, y, window).await
    }

    /// Create a pane.
    ///
    /// * `window: None` → hollow orphan pane (no link, no window).
    /// * `window: Some(wid)` mirroring a tmux window → split inside that real
    ///   window through the session's control client; the row is the one
    ///   reconciliation creates, whose Buffer origin retains the `%N` on the
    ///   session's existing link (no link is created — a tmux session has
    ///   exactly one).
    /// * `window: Some(wid)` native → hollow pane inside the window.
    ///
    /// v0.6.0 (ADR 0010): the pane belongs to `session`, and the window it is
    /// put into must belong there too (arch.md §8).
    // The argument list is the model's (geometry + window + session); grouping
    // it into a struct would just move the same fields somewhere else.
    #[allow(clippy::too_many_arguments)]
    pub async fn add_pane_in(
        &self,
        session: SessionId,
        name: Option<String>,
        cols: u16,
        rows: u16,
        x: u16,
        y: u16,
        window: Option<WindowId>,
    ) -> Result<PaneId> {
        self.require_session(session).await?;

        // In a tmux session, a pane with no window would be a row no byte could
        // ever reach: `link add` and `pane attach` are both refused there. So
        // "no window" means "the one tmux is showing", exactly like
        // `split-window` with no target.
        let window = match window {
            Some(w) => Some(w),
            None => {
                if let SessionBackend::Tmux { session: name, .. } =
                    self.get_session(session).await?.backend
                {
                    let lines = self
                        .tmux_send(
                            session,
                            &format!("display-message -p -t {name} '#{{window_id}}'"),
                        )
                        .await?;
                    let handle = tmux_handle_from(&lines, '@').ok_or_else(|| {
                        Error::Sink(format!("tmux did not report the current window: {lines:?}"))
                    })?;
                    let wid = self
                        .ui
                        .windows()
                        .await
                        .into_iter()
                        .find(|w| {
                            w.info.session_id == session
                                && w.info.backend.tmux_handle() == Some(handle.as_str())
                        })
                        .map(|w| w.info.id)
                        .ok_or_else(|| Error::Sink(format!("tmux window {handle} is not a row")))?;
                    Some(wid)
                } else {
                    None
                }
            }
        };

        // A tmux window's panes are tmux's: split it and adopt what tmux
        // creates, rather than inventing a hollow row that no byte could reach.
        if let Some(wid) = window {
            let arc_window = self
                .ui
                .window(wid)
                .await
                .ok_or(Error::WindowNotFound(wid))?;
            if arc_window.info.session_id != session {
                return Err(Error::NotInSession {
                    session,
                    what: "window",
                    id: wid.to_string(),
                });
            }
            if let Some(handle) = arc_window.info.backend.tmux_handle().map(str::to_string) {
                let lines = self
                    .tmux_send(session, &crate::infra::tmux::split_window_command(&handle))
                    .await?;
                let key = tmux_handle_from(&lines, '%').ok_or_else(|| {
                    Error::Sink(format!(
                        "tmux split-window did not report a pane handle: {lines:?}"
                    ))
                })?;
                self.refresh_window(session, &handle).await?;
                let existing = self
                    .pane_for_tmux_handle(session, &key)
                    .await
                    .ok_or_else(|| {
                        Error::Sink(format!(
                            "tmux created pane {key} but it did not become a row"
                        ))
                    })?;
                if let Some(requested) = name {
                    let mut renamed = existing.clone();
                    renamed.name = requested;
                    self.ui.put_pane(renamed).await;
                    self.save_state().await?;
                }
                return Ok(existing.id);
            }
        }

        let pane_id = PaneId::new();
        let info = PaneInfo {
            id: pane_id,
            session_id: session,
            name: name.unwrap_or_else(|| format!("p-{pane_id}")),
            created_at: chrono::Utc::now(),
            cols,
            rows,
            x,
            y,
            buffer_id: None,
            window_id: window,
        };
        self.ui.put_pane(info).await;
        if let Some(wid) = window {
            if let Some(arc_window) = self.ui.window(wid).await {
                arc_window.panes.lock().await.push(pane_id);
            }
        }
        self.save_state().await?;
        Ok(pane_id)
    }

    pub async fn del_pane(&self, id: PaneId) -> Result<()> {
        let info = self.ui.pane(id).await.ok_or(Error::PaneNotFound(id))?;
        if let Some(wid) = info.window_id {
            if let Some(arc_window) = self.ui.window(wid).await {
                arc_window.panes.lock().await.retain(|x| *x != id);
            }
        }
        self.ui.remove_pane(id).await;
        self.save_state().await?;
        Ok(())
    }

    pub async fn del_buffer(&self, id: BufferId) -> Result<()> {
        let info = self
            .buffer_service
            .get(id)
            .await
            .ok_or_else(|| Error::Invalid(format!("buffer not found: {id}")))?;
        let runtime = self.buffer_service.take_runtime(id).await;
        if let Some(runtime) = runtime {
            let _ = runtime.close().await;
        }
        self.buffer_service.remove_memory(id).await;
        self.buffer_service.remove(id).await;
        if let Some(client) = &self.daemon {
            let _ = client.delete_buffer(id).await;
        }
        for mut pane in self.ui.panes().await {
            if pane.buffer_id == Some(id) {
                pane.buffer_id = None;
                self.ui.put_pane(pane).await;
            }
        }
        self.rebuild_backend(info.session_id).await?;
        self.save_state().await?;
        Ok(())
    }

    // ===== pane UI to buffer association =====

    /// Point a UI pane at the buffer produced by a raw link.
    pub async fn attach_pane(&self, pid: PaneId, lid: LinkId) -> Result<()> {
        let link = self
            .link_service
            .get(lid)
            .await
            .ok_or(Error::LinkNotFound(lid))?;
        let pane = self.ui.pane(pid).await.ok_or(Error::PaneNotFound(pid))?;
        if pane.session_id != link.info.session_id {
            return Err(Error::NotInSession {
                session: link.info.session_id,
                what: "pane",
                id: pid.to_string(),
            });
        }
        if link.info.is_control() {
            return Err(Error::Invalid(format!(
                "link {lid} is a tmux control link; pane {pid} must reference the buffer \
                 created for a tmux pane handle"
            )));
        }
        let buffer_id = self
            .backends
            .lock()
            .await
            .get(&pane.session_id)
            .and_then(|manager| manager.buffer_for_link(lid))
            .ok_or_else(|| Error::Invalid(format!("link {lid} produced no buffer")))?;
        let buffer = self
            .buffer_service
            .get(buffer_id)
            .await
            .ok_or_else(|| Error::Invalid(format!("buffer {buffer_id} not found")))?;

        if let Some(client) = &self.daemon {
            if client.attach_buffer(buffer.id, lid).await.is_err() {
                client.ensure().await?;
                client.create_link(lid, pipe_transport(&link.info)?).await?;
                client.create_buffer(buffer.id).await?;
                client.attach_buffer(buffer.id, lid).await?;
            }
            let runtime: Arc<BufferRuntime> =
                Arc::new(BufferRuntime::daemon(Arc::clone(client), buffer.id));
            self.buffer_service.put_runtime(buffer.id, runtime).await;
        }

        let mut updated = pane;
        updated.buffer_id = Some(buffer.id);
        self.ui.put_pane(updated).await;
        self.save_state().await?;
        Ok(())
    }

    /// Remove the UI association. The buffer and backend pump keep running.
    pub async fn detach_pane(&self, pid: PaneId) -> Result<()> {
        self.ui.clear_pane_buffer(pid).await?;
        self.save_state().await?;
        Ok(())
    }

    // ===== window ⇄ pane association =====

    pub async fn add_pane_to_window(&self, wid: WindowId, pid: PaneId) -> Result<()> {
        self.ui.add_pane_to_window(wid, pid).await?;
        self.save_state().await?;
        Ok(())
    }

    pub async fn remove_pane_from_window(&self, wid: WindowId, pid: PaneId) -> Result<()> {
        self.ui.remove_pane_from_window(wid, pid).await?;
        self.save_state().await?;
        Ok(())
    }

    pub async fn move_pane_to_window(&self, pid: PaneId, wid: WindowId) -> Result<()> {
        self.ui.move_pane_to_window(pid, wid).await?;
        self.save_state().await?;
        Ok(())
    }

    // ===== geometry updates =====

    pub async fn resize_pane(&self, pid: PaneId, cols: u16, rows: u16) -> Result<()> {
        let info = self.ui.pane(pid).await.ok_or(Error::PaneNotFound(pid))?;
        // Resize the real thing first. If the transport has no size (serial) or
        // the call fails, nothing has changed yet — a failed resize must not
        // leave a modified `PaneInfo` behind, which is the same silent lie as
        // returning Ok without doing anything.
        if let Some(buffer_id) = info.buffer_id {
            if self.buffer_service.runtime(buffer_id).await.is_some() {
                self.buffer_service.resize(buffer_id, cols, rows).await?;
            }
        }
        self.ui.set_pane_size(pid, cols, rows).await?;
        self.save_state().await?;
        Ok(())
    }

    pub async fn move_pane(&self, pid: PaneId, x: u16, y: u16) -> Result<()> {
        self.ui.set_pane_position(pid, x, y).await?;
        self.save_state().await?;
        Ok(())
    }
}
