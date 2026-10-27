//! In-process tmux control-client runtime owned by the backend layer.

use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::{Mutex, MutexGuard};

use crate::infra::tmux::TmuxControl;
use crate::service::session_service::SessionId;

#[derive(Default)]
pub(crate) struct TmuxRuntime {
    controls: Mutex<HashMap<SessionId, Arc<TmuxControl>>>,
    reconcile: Mutex<()>,
    attach: Mutex<()>,
}

impl TmuxRuntime {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) async fn control(&self, session: SessionId) -> Option<Arc<TmuxControl>> {
        self.controls.lock().await.get(&session).cloned()
    }

    pub(crate) async fn put_control(&self, session: SessionId, control: Arc<TmuxControl>) {
        self.controls.lock().await.insert(session, control);
    }

    pub(crate) async fn take_control(&self, session: SessionId) -> Option<Arc<TmuxControl>> {
        self.controls.lock().await.remove(&session)
    }

    pub(crate) async fn attach_guard(&self) -> MutexGuard<'_, ()> {
        self.attach.lock().await
    }

    pub(crate) async fn reconcile_guard(&self) -> MutexGuard<'_, ()> {
        self.reconcile.lock().await
    }
}
