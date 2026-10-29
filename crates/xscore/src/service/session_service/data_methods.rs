use super::*;
use crate::service::buffer_service::{BufferId, BufferInfo, BufferOrigin, BufferRuntime};
use crate::service::link_service::LinkId;
use crate::service::session_service::SessionId;
use crate::ui::PaneId;
use crate::{Error, Result};
use std::sync::Arc;

impl SessionService {
    // ===== data ops (read / write / resize via runtime) =====

    pub async fn data_read(&self, pid: PaneId, buf: &mut [u8]) -> Result<usize> {
        let buffer_id = self.ensure_buffer_runtime(pid).await?;
        self.buffer_service.read(buffer_id, buf).await
    }

    pub async fn data_write(&self, pid: PaneId, data: &[u8]) -> Result<usize> {
        let buffer_id = self.ensure_buffer_runtime(pid).await?;
        self.buffer_service.write(buffer_id, data).await
    }

    /// Resize a pane's runtime AND its info (cols/rows). Errors with
    /// `NotAttached(pid)` for orphan panes — use `resize_pane` if you
    /// want to update geometry without going through the runtime.
    pub async fn data_resize(&self, pid: PaneId, cols: u16, rows: u16) -> Result<()> {
        let buffer_id = self.ensure_buffer_runtime(pid).await?;
        self.buffer_service.resize(buffer_id, cols, rows).await?;
        if let Some(mut info) = self.ui.pane(pid).await {
            info.cols = cols;
            info.rows = rows;
            self.ui.put_pane(info).await;
        }
        self.save_state().await?;
        Ok(())
    }

    /// Resolve a pane's live runtime, distinguishing the two ways it can
    /// be missing:
    ///
    /// * `NotAttached(pid)` — the pane has no link binding at all
    ///   (orphan). The user must `pane attach`.
    /// * `LinkNotConnected(lid)` — the pane IS bound, but the link's
    ///   transport died with the process that owned it (only possible
    ///   for direct links, and only after a restart). The binding is
    ///   real; the connection is not. Re-adding the link reconnects.
    ///
    /// v0.4.1 (P0-3): before this split, both cases surfaced as
    /// `NotAttached`, which told the user their still-bound pane had
    /// never been attached — persisted state and runtime disagreed.
    ///
    /// v0.5.0 (ADR 0009): with the daemon enabled, direct panes get a
    /// `daemon BufferRuntime` **on demand** instead of only right after an
    /// `attach`. That is what makes `data read/write` work from a process
    /// that only loaded `state.json` — the transport and the buffer were
    /// never in this process to begin with.
    pub(super) async fn ensure_buffer_runtime(&self, pid: PaneId) -> Result<BufferId> {
        let pane = self.ui.pane(pid).await.ok_or(Error::PaneNotFound(pid))?;
        let buffer_id = pane.buffer_id.ok_or(Error::NotAttached(pid))?;
        if self.buffer_service.runtime(buffer_id).await.is_some() {
            return Ok(buffer_id);
        }
        let buffer = self
            .buffer_service
            .get(buffer_id)
            .await
            .ok_or_else(|| Error::Invalid(format!("buffer {buffer_id} not found")))?;
        let runtime: Arc<BufferRuntime> = match buffer.origin.clone() {
            BufferOrigin::Tmux {
                link_id,
                pane_handle,
            } => self.control_runtime(&buffer, link_id, &pane_handle).await?,
            BufferOrigin::Link { link_id } => self.raw_runtime(&buffer, link_id).await?,
        };
        self.buffer_service.put_runtime(buffer_id, runtime).await;
        Ok(buffer_id)
    }

    /// The runtime for a pane that lives on its session's control link, if it
    /// does.
    ///
    /// Attaching here — rather than only at creation — is what makes
    /// `data read/write` work from a process that did nothing but load
    /// `state.json` (arch.md §7: the structure is live, not a mirror).
    ///
    /// v0.6.0 PR5: with a daemon, the read path is the **daemon's** buffer, so
    /// the daemon must hold the client (it may have restarted) and a subscriber
    /// for this `%N` before the runtime is handed out. Both calls are
    /// idempotent, and the second is what makes a pane discovered late
    /// (arch.md §13: no history) start streaming from now on.
    pub(super) async fn control_runtime(
        &self,
        info: &BufferInfo,
        link_id: LinkId,
        key: &str,
    ) -> Result<Arc<BufferRuntime>> {
        let Some(link) = self.link_service.get(link_id).await else {
            return Err(Error::LinkNotConnected(link_id));
        };
        if !link.info.is_control() {
            return Err(Error::LinkNotConnected(link_id));
        }
        match &self.daemon {
            Some(client) => {
                if !client.probe_tmux(info.session_id).await {
                    let target = self.tmux_target(info.session_id).await?;
                    client
                        .open_tmux(
                            info.session_id,
                            target.host,
                            target.socket,
                            &target.name,
                            target.cols,
                            target.rows,
                        )
                        .await?;
                }
                client
                    .attach_control_buffer(info.session_id, info.id, key)
                    .await?;
                let runtime: Arc<BufferRuntime> =
                    Arc::new(BufferRuntime::daemon(Arc::clone(client), info.id));
                Ok(runtime)
            }
            None => {
                let control = self.ensure_local_control(info.session_id).await?;
                let buffer = self.buffer_service.ring(info.id).await;
                let runtime: Arc<BufferRuntime> =
                    Arc::new(BufferRuntime::control(control, key.to_string(), buffer));
                Ok(runtime)
            }
        }
    }

    pub(super) async fn raw_runtime(
        &self,
        info: &BufferInfo,
        link_id: LinkId,
    ) -> Result<Arc<BufferRuntime>> {
        if let Some(client) = &self.daemon {
            let link_live = client.probe_link(link_id).await.unwrap_or(false);
            let buffer_live = client.probe_buffer(info.id).await.unwrap_or(false);
            if !link_live && !buffer_live {
                return Err(Error::LinkNotConnected(link_id));
            }
            if link_live {
                client.attach_buffer(info.id, link_id).await?;
            }
            return Ok(Arc::new(BufferRuntime::daemon(Arc::clone(client), info.id)));
        }
        let link = self
            .link_service
            .get(link_id)
            .await
            .ok_or(Error::LinkNotConnected(link_id))?;
        if !link.connected {
            return Err(Error::LinkNotConnected(link_id));
        }
        let buffer = self.buffer_service.ring(info.id).await;
        Ok(Arc::new(BufferRuntime::local(
            Arc::clone(&link.transport),
            buffer,
        )))
    }

    /// Forget a Buffer everywhere: runtime, in-process ring, registry and
    /// daemon slot. Pane references are intentionally left to the caller.
    pub(super) async fn discard_buffer(&self, buffer_id: BufferId) {
        self.buffer_service.forget(buffer_id).await;
        if let Some(client) = &self.daemon {
            let _ = client.delete_buffer(buffer_id).await;
        }
    }

    pub(super) async fn discard_buffers_for_session(&self, session_id: SessionId) {
        for buffer in self.buffer_service.list_for_session(session_id).await {
            self.discard_buffer(buffer.id).await;
        }
    }
}
