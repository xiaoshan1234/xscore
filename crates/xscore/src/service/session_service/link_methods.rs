use super::*;
use crate::infra::link::factory as transport_factory;
use crate::infra::link::{ConnectSpec, LinkAddress};
use crate::service::buffer_service::{BufferId, BufferInfo, BufferOrigin, BufferRuntime};
use crate::service::link_service::{EndpointRef, Link, LinkId};
use crate::service::session_service::SessionId;
use crate::ui::window::Window;
use crate::ui::{PaneId, PaneInfo, WindowBackend, WindowId, WindowInfo};
use crate::{Error, Result};
use std::sync::Arc;

impl SessionService {
    pub async fn add_link(
        &self,
        spec: ConnectSpec,
        address: LinkAddress,
        id: Option<LinkId>,
        pane: Option<PaneId>,
    ) -> Result<LinkAddResult> {
        let sid = self.resolve_session(None).await?;
        self.add_link_in(sid, spec, address, id, pane).await
    }

    /// v0.6.0 (ADR 0010): `add_link`, scoped to an explicit session. Every
    /// window/pane it creates carries `session`, and a pane it binds must
    /// already belong to the same session (arch.md §8).
    pub async fn add_link_in(
        &self,
        session: SessionId,
        spec: ConnectSpec,
        address: LinkAddress,
        id: Option<LinkId>,
        pane: Option<PaneId>,
    ) -> Result<LinkAddResult> {
        self.require_session(session).await?;
        // ADR 0010 §4: a tmux session has exactly one link — the control
        // client. A second one would be a second mechanism driving the same
        // server session.
        if let Some(existing) = self.sessions.get(session).await {
            if existing.info.is_tmux() {
                return Err(Error::TmuxSessionHasOneLink(session));
            }
        }
        // --- validate before allocating anything (a rejected call must not
        //     leave a spawned child behind) ---
        if let Some(pid) = pane {
            let info = self.ui.pane(pid).await.ok_or(Error::PaneNotFound(pid))?;
            if info.session_id != session {
                // Cross-session binding is never implicit (arch.md §8).
                return Err(Error::NotInSession {
                    session,
                    what: "pane",
                    id: pid.to_string(),
                });
            }
        }

        let link_id = match id {
            Some(supplied) if self.link_service.get(supplied).await.is_some() => {
                return Err(Error::LinkIdExists(supplied));
            }
            Some(supplied) => supplied,
            None => LinkId::new(),
        };

        // v0.5.0 (ADR 0009): with the daemon enabled, a pipe link's REAL
        // transport is created inside the daemon and the CLI only keeps a
        // handle to it. That is what makes the link outlive this process.
        let daemon = self.daemon.clone();
        let buffer_id = BufferId::new();
        if let Some(client) = &daemon {
            client.ensure().await?;
            client.create_link(link_id, spec.transport.clone()).await?;
            if let Err(error) = client.create_buffer(buffer_id).await {
                let _ = client.delete_link(link_id).await;
                return Err(error);
            }
        }

        let transport_arc: Arc<dyn crate::infra::link::Transport> = match &daemon {
            Some(_) => Arc::new(crate::service::daemon::DaemonTransport::new(link_id)),
            None => {
                // The transport IS the byte source of a pipe link, so it is
                // opened once here (the other callers of `transport_for` own
                // their own lifetime): a runtime may then read and write without
                // a second handshake. The deleted sink factory used to do this
                // for the raw case; PR4 moved the one call that mattered here.
                let mut transport =
                    transport_factory::transport_for(spec.transport.clone()).await?;
                transport.open().await?;
                Arc::from(transport)
            }
        };
        self.buffer_service
            .insert(BufferInfo {
                id: buffer_id,
                session_id: session,
                name: format!("b-{}", &buffer_id.to_string()[..8]),
                created_at: chrono::Utc::now(),
                origin: BufferOrigin::Link { link_id },
            })
            .await;

        // --- branch: decide the window / pane / runtime ---
        type Branch = (
            Option<WindowId>,
            PaneId,
            PaneInfo,
            Option<Arc<BufferRuntime>>,
            Option<Window>,
        );
        let (window_id, pane_id, pane_record, runtime, window_record): Branch = match pane {
            // (a) bind an existing pane: no window is created — the pane keeps
            //     whatever window it had.
            Some(pid) => {
                let runtime: Arc<BufferRuntime> = match &daemon {
                    Some(client) => Arc::new(BufferRuntime::daemon(Arc::clone(client), buffer_id)),
                    None => {
                        let buf = self.buffer_for(buffer_id).await;
                        Arc::new(BufferRuntime::local(Arc::clone(&transport_arc), buf))
                    }
                };
                let mut info = self.ui.pane(pid).await.ok_or(Error::PaneNotFound(pid))?;
                info.buffer_id = Some(buffer_id);
                (None, pid, info, Some(runtime), None)
            }

            // (b) a pipe link with no pane: native window + first pane.
            None => {
                let wid = WindowId::new();
                let pid = PaneId::new();
                let w = Window::new(WindowInfo {
                    id: wid,
                    session_id: session,
                    name: format!("w-{}", &wid.to_string()[..8]),
                    created_at: chrono::Utc::now(),
                    cols: 120,
                    rows: 30,
                    backend: WindowBackend::Native,
                });
                let _ = w.add_pane(pid).await;
                let runtime: Arc<BufferRuntime> = match &daemon {
                    Some(client) => Arc::new(BufferRuntime::daemon(Arc::clone(client), buffer_id)),
                    None => {
                        let buf = self.buffer_for(buffer_id).await;
                        Arc::new(BufferRuntime::local(Arc::clone(&transport_arc), buf))
                    }
                };
                (
                    Some(wid),
                    pid,
                    PaneInfo {
                        id: pid,
                        session_id: session,
                        name: format!("p-{pid}"),
                        created_at: chrono::Utc::now(),
                        cols: 80,
                        rows: 24,
                        x: 0,
                        y: 0,
                        buffer_id: Some(buffer_id),
                        window_id: Some(wid),
                    },
                    Some(runtime),
                    Some(w),
                )
            }
        };

        let link_info = crate::service::link_service::LinkInfo {
            id: link_id,
            session_id: session,
            name: if spec.name.is_empty() {
                link_id.to_string()
            } else {
                spec.name.clone()
            },
            created_at: chrono::Utc::now(),
            address: address.clone(),
            // Every link this method creates is a pipe: one byte stream, at
            // most one reader. A `Control` link is created only by
            // `open_control_link` (ADR 0010 §D2).
            endpoint: EndpointRef::Pipe(spec.transport.clone()),
        };

        let link = Link {
            id: link_id,
            info: link_info,
            transport: transport_arc,
            control: None,
            connected: true,
        };
        self.link_service.insert(link).await;
        if let Some(w) = window_record {
            self.ui.put_window(w).await;
        }
        self.ui.put_pane(pane_record).await;
        if let Some(b) = runtime {
            self.buffer_service.put_runtime(buffer_id, b).await;
        }

        // ADR 0009: the binding only becomes LIVE at Attach — the daemon
        // starts the pane's pump here, and keeps it running after this
        // process exits. A failure here must not leave an orphaned
        // transport behind, so tear the daemon link back down.
        if let Some(client) = &daemon {
            if let Err(e) = client.attach_buffer(buffer_id, link_id).await {
                let _ = client.delete_link(link_id).await;
                let _ = client.delete_buffer(buffer_id).await;
                self.buffer_service.remove(buffer_id).await;
                return Err(e);
            }
        }

        self.rebuild_backend(session).await?;
        if let Err(e) = self.save_state().await {
            // A failed save means the CLI exits with an error, so nothing
            // will ever reference this link. Do not leave its transport
            // (and a daemon that can therefore never idle-exit) behind.
            if let Some(client) = &daemon {
                let _ = client.delete_link(link_id).await;
                let _ = client.delete_buffer(buffer_id).await;
            }
            self.buffer_service.remove(buffer_id).await;
            let _ = self.rebuild_backend(session).await;
            return Err(e);
        }
        Ok(LinkAddResult {
            link_id,
            buffer_id,
            window_id,
            pane_id,
        })
    }

    /// Full teardown of a link.
    ///
    /// * pipe link: stop feeding its buffers, close the transport once, and
    ///   remove the link. Buffers and their UI panes remain.
    /// * control link: refused (ADR 0010 §D5) — see `del_session` /
    ///   `detach_session` for the two verbs that do tear a tmux session down.
    pub async fn del_link(&self, id: LinkId) -> Result<()> {
        let arc_link = self
            .link_service
            .get(id)
            .await
            .ok_or(Error::LinkNotFound(id))?;

        // ADR 0010 §D5: every pane in a tmux session routes through its control
        // link, so deleting it would leave them with buffers and no way back.
        if arc_link.info.is_control() {
            return Err(Error::ControlLinkNotDeletable(id));
        }

        let linked_buffers: Vec<BufferId> = self
            .buffer_service
            .list()
            .await
            .into_iter()
            .filter_map(|buffer| match buffer.origin {
                BufferOrigin::Link { link_id } if link_id == id => Some(buffer.id),
                _ => None,
            })
            .collect();
        for buffer_id in linked_buffers {
            let runtime = self.buffer_service.take_runtime(buffer_id).await;
            if let Some(b) = runtime {
                // local BufferRuntime::close() stops the pump only (in daemon
                // mode: Detach), so the transport below is closed
                // exactly once, on purpose.
                let _ = b.close().await;
            }
        }
        match &self.daemon {
            // ADR 0009: the daemon owns the transport. DeleteLink
            // closes it once and unbinds its panes — but keeps their
            // slots and buffers, so the history is still readable and
            // a later re-attach resumes where it left off.
            Some(client) => {
                // Best effort: if the daemon is down the transport is
                // already gone, which is the state we wanted anyway.
                let _ = client.delete_link(id).await;
            }
            None => {
                arc_link.transport.close().await?;
            }
        }

        self.link_service.remove(id).await;
        self.rebuild_backend(arc_link.info.session_id).await?;
        self.save_state().await?;
        Ok(())
    }
}
