use super::*;
use crate::infra::link::NoopTransport;
use crate::service::link_service::Link;
use crate::service::session_service::SessionId;
use crate::ui::window::Window;
use crate::{Error, Result};
use std::sync::Arc;

impl SessionService {
    pub async fn save_state(&self) -> Result<()> {
        // v0.6.0 (ADR 0010 D8): `sessions` first — the children below all
        // reference one. `list_sessions` sorts by `created_at`, so the file is
        // stable across saves rather than following HashMap iteration order.
        let sessions = self.list_sessions().await?;

        let arc_links = self.link_service.list().await;
        let links: Vec<crate::service::link_service::LinkInfo> =
            arc_links.iter().map(|l| l.info.clone()).collect();

        let arc_windows = self.ui.windows().await;
        let mut windows: Vec<crate::service::persistence::WindowSnapshot> = Vec::new();
        for w in &arc_windows {
            let pane_ids = w.panes.lock().await.clone();
            windows.push(crate::service::persistence::WindowSnapshot {
                info: w.info.clone(),
                panes: pane_ids,
            });
        }

        let panes = self.ui.panes().await;
        let buffers = self.buffer_service.list().await;

        let snapshot = crate::service::persistence::StateSnapshot {
            schema_version: crate::service::persistence::SCHEMA_VERSION,
            sessions,
            links,
            buffers,
            windows,
            panes,
        };
        tokio::task::spawn_blocking(move || crate::service::persistence::save(&snapshot))
            .await
            .map_err(|e| Error::Config(format!("join: {e}")))?
    }

    pub async fn load_from_state() -> Result<Self> {
        match crate::service::persistence::load()? {
            Some(s) => Self::hydrate(s).await,
            None => Ok(Self::new()),
        }
    }

    pub(super) async fn hydrate(
        snapshot: crate::service::persistence::StateSnapshot,
    ) -> Result<Self> {
        let mgr = Self::new();
        // v0.6.0 (ADR 0010 D8): sessions BEFORE their children — a link's
        // `session_id` must resolve, and there is deliberately no fallback that
        // invents a session on load.
        for info in snapshot.sessions {
            mgr.sessions.insert(Session::new(info)).await;
        }
        for info in snapshot.buffers {
            if mgr.sessions.get(info.session_id).await.is_some() {
                mgr.buffer_service.insert(info).await;
            }
        }
        for info in snapshot.links {
            if mgr.sessions.get(info.session_id).await.is_none() {
                tracing::warn!(
                    "hydration: skipping link {} — no session {} in state",
                    info.id,
                    info.session_id
                );
                continue;
            }
            mgr.hydrate_link(info).await;
        }
        for ws in snapshot.windows {
            if mgr.sessions.get(ws.info.session_id).await.is_none() {
                tracing::warn!(
                    "hydration: skipping window {} — no session {} in state",
                    ws.info.id,
                    ws.info.session_id
                );
                continue;
            }
            let w = Window::new(ws.info);
            for pid in ws.panes {
                w.panes.lock().await.push(pid);
            }
            mgr.ui.put_window(w).await;
        }
        for pinfo in snapshot.panes {
            if mgr.sessions.get(pinfo.session_id).await.is_none() {
                tracing::warn!(
                    "hydration: skipping pane {} — no session {} in state",
                    pinfo.id,
                    pinfo.session_id
                );
                continue;
            }
            mgr.ui.put_pane(pinfo).await;
        }
        let session_ids: Vec<SessionId> = mgr
            .sessions
            .list()
            .await
            .into_iter()
            .map(|session| session.info.id)
            .collect();
        for session_id in session_ids {
            if let Err(e) = mgr.rebuild_backend(session_id).await {
                tracing::warn!("hydration: session manager for {session_id}: {e}");
            }
        }
        // Nothing else is rebuilt here on purpose. A pipe link's transport died
        // with the process that owned it (see `Link::connected`: false is the
        // honest answer, so data ops say `LinkNotConnected`), and a control
        // link has no client here at all — in daemon mode the daemon holds it
        // (and the buffers), otherwise `ensure_local_control` spawns one on
        // first use. Either way a fresh CLI process works against a session it
        // never created.
        Ok(mgr)
    }

    /// A hydrated link has no live connection yet: the transport of a pipe died
    /// with the previous process, and a control link's client is spawned on
    /// first use. Recording that honestly is what lets data ops report
    /// `LinkNotConnected` instead of the misleading `NotAttached`.
    pub(super) async fn hydrate_link(&self, info: crate::service::link_service::LinkInfo) {
        let link = Link {
            id: info.id,
            info,
            transport: Arc::new(NoopTransport),
            control: None,
            connected: false,
        };
        self.link_service.insert(link).await;
    }
}
