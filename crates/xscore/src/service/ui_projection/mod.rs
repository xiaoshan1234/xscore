//! Coordinates the UI registries without owning bytes or connections.

use std::sync::Arc;

use crate::ui::pane_registry::PaneRegistry;
use crate::ui::window::Window;
use crate::ui::window_registry::WindowRegistry;
use crate::ui::{PaneId, PaneInfo, WindowId};
use crate::{Error, Result};

#[derive(Clone, Default)]
pub struct UiProjection {
    windows: Arc<WindowRegistry>,
    panes: Arc<PaneRegistry>,
}

impl UiProjection {
    pub fn new() -> Self {
        Self::default()
    }

    pub async fn pane(&self, id: PaneId) -> Option<PaneInfo> {
        self.panes.get(id).await
    }

    pub async fn panes(&self) -> Vec<PaneInfo> {
        self.panes.list_all().await
    }

    pub async fn put_pane(&self, pane: PaneInfo) {
        self.panes.insert(pane).await;
    }

    pub async fn remove_pane(&self, id: PaneId) -> Option<PaneInfo> {
        self.panes.remove(id).await
    }

    pub async fn window(&self, id: WindowId) -> Option<Arc<Window>> {
        self.windows.get(id).await
    }

    pub async fn windows(&self) -> Vec<Arc<Window>> {
        self.windows.list_all().await
    }

    pub async fn put_window(&self, window: Window) -> Arc<Window> {
        self.windows.insert(window).await
    }

    pub async fn remove_window(&self, id: WindowId) -> Option<Arc<Window>> {
        self.windows.remove(id).await
    }

    /// Remove a window row and unlink its panes. Panes survive as orphans.
    pub async fn delete_window(&self, id: WindowId) -> Result<()> {
        let window = self.window(id).await.ok_or(Error::WindowNotFound(id))?;
        let pane_ids = window.panes.lock().await.clone();
        for pane_id in pane_ids {
            if let Some(mut pane) = self.pane(pane_id).await {
                pane.window_id = None;
                self.put_pane(pane).await;
            }
        }
        self.remove_window(id).await;
        Ok(())
    }

    pub async fn add_pane_to_window(&self, window: WindowId, pane: PaneId) -> Result<()> {
        let target = self
            .window(window)
            .await
            .ok_or(Error::WindowNotFound(window))?;
        let mut info = self.pane(pane).await.ok_or(Error::PaneNotFound(pane))?;
        if target.info.session_id != info.session_id {
            return Err(Error::NotInSession {
                session: target.info.session_id,
                what: "pane",
                id: pane.to_string(),
            });
        }
        if target.panes.lock().await.contains(&pane) {
            return Err(Error::AlreadyInWindow { window, pane });
        }
        info.window_id = Some(window);
        self.put_pane(info).await;
        target.panes.lock().await.push(pane);
        Ok(())
    }

    pub async fn remove_pane_from_window(&self, window: WindowId, pane: PaneId) -> Result<()> {
        let target = self
            .window(window)
            .await
            .ok_or(Error::WindowNotFound(window))?;
        self.pane(pane).await.ok_or(Error::PaneNotFound(pane))?;
        {
            let mut panes = target.panes.lock().await;
            if !panes.contains(&pane) {
                return Err(Error::NotInWindow { window, pane });
            }
            panes.retain(|candidate| *candidate != pane);
        }
        if let Some(mut info) = self.pane(pane).await {
            info.window_id = None;
            self.put_pane(info).await;
        }
        Ok(())
    }

    pub async fn move_pane_to_window(&self, pane: PaneId, window: WindowId) -> Result<()> {
        let info = self.pane(pane).await.ok_or(Error::PaneNotFound(pane))?;
        let target = self
            .window(window)
            .await
            .ok_or(Error::WindowNotFound(window))?;
        if target.info.session_id != info.session_id {
            return Err(Error::NotInSession {
                session: target.info.session_id,
                what: "pane",
                id: pane.to_string(),
            });
        }
        if let Some(current) = info.window_id {
            if current != window {
                self.remove_pane_from_window(current, pane).await?;
            }
        }
        self.add_pane_to_window(window, pane).await
    }

    pub async fn clear_pane_buffer(&self, pane: PaneId) -> Result<()> {
        let mut info = self.pane(pane).await.ok_or(Error::PaneNotFound(pane))?;
        if info.buffer_id.take().is_none() {
            return Err(Error::NotAttached(pane));
        }
        self.put_pane(info).await;
        Ok(())
    }

    pub async fn set_pane_size(&self, pane: PaneId, cols: u16, rows: u16) -> Result<()> {
        let mut info = self.pane(pane).await.ok_or(Error::PaneNotFound(pane))?;
        info.cols = cols;
        info.rows = rows;
        self.put_pane(info).await;
        Ok(())
    }

    pub async fn set_pane_position(&self, pane: PaneId, x: u16, y: u16) -> Result<()> {
        let mut info = self.pane(pane).await.ok_or(Error::PaneNotFound(pane))?;
        info.x = x;
        info.y = y;
        self.put_pane(info).await;
        Ok(())
    }

    /// Remove all UI rows owned by one Session.
    pub async fn remove_session_rows(
        &self,
        session_id: crate::service::session_service::SessionId,
    ) {
        let pane_ids: Vec<PaneId> = self
            .panes()
            .await
            .into_iter()
            .filter(|pane| pane.session_id == session_id)
            .map(|pane| pane.id)
            .collect();
        for pane_id in pane_ids {
            self.remove_pane(pane_id).await;
        }

        let window_ids: Vec<WindowId> = self
            .windows()
            .await
            .iter()
            .filter(|window| window.info.session_id == session_id)
            .map(|window| window.info.id)
            .collect();
        for window_id in window_ids {
            self.remove_window(window_id).await;
        }
    }
}
