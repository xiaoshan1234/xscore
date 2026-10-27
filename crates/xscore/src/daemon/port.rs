//! Daemon implementation of the service outbound runtime port.

use std::sync::Arc;

use async_trait::async_trait;

use crate::daemon::client::{self, DaemonClient, DaemonTransport};
use crate::infra::link::{Transport, TransportConfig};
use crate::infra::tmux::{TmuxHostConfig, TmuxSocket};
use crate::service::backend::daemon_port::DaemonPort;
use crate::service::backend::tmux_model::TmuxStructure;
use crate::service::buffer_service::BufferId;
use crate::service::link_service::LinkId;
use crate::service::session_service::SessionId;
use crate::Result;

#[async_trait]
impl DaemonPort for DaemonClient {
    async fn ensure(&self) -> Result<()> {
        DaemonClient::ensure(self).await
    }

    async fn create_link(&self, link: LinkId, transport: TransportConfig) -> Result<()> {
        DaemonClient::create_link(self, link, transport).await
    }

    async fn delete_link(&self, link: LinkId) -> Result<()> {
        DaemonClient::delete_link(self, link).await
    }

    async fn create_buffer(&self, buffer: BufferId) -> Result<()> {
        DaemonClient::create_buffer(self, buffer).await
    }

    async fn delete_buffer(&self, buffer: BufferId) -> Result<()> {
        DaemonClient::delete_buffer(self, buffer).await
    }

    async fn attach_buffer(&self, buffer: BufferId, link: LinkId) -> Result<()> {
        DaemonClient::attach_buffer(self, buffer, link).await
    }

    async fn attach_control_buffer(
        &self,
        session: SessionId,
        buffer: BufferId,
        key: &str,
    ) -> Result<()> {
        DaemonClient::attach_control_buffer(self, session, buffer, key).await
    }

    async fn detach(&self, buffer: BufferId) -> Result<()> {
        DaemonClient::detach(self, buffer).await
    }

    async fn read(&self, buffer: BufferId, max: usize) -> Result<Vec<u8>> {
        DaemonClient::read(self, buffer, max).await
    }

    async fn write(&self, buffer: BufferId, data: &[u8]) -> Result<usize> {
        DaemonClient::write(self, buffer, data).await
    }

    async fn resize(&self, buffer: BufferId, cols: u16, rows: u16) -> Result<()> {
        DaemonClient::resize(self, buffer, cols, rows).await
    }

    async fn open_tmux(
        &self,
        session: SessionId,
        host: TmuxHostConfig,
        socket: TmuxSocket,
        name: &str,
        cols: u16,
        rows: u16,
    ) -> Result<TmuxStructure> {
        DaemonClient::open_tmux(self, session, host, socket, name, cols, rows).await
    }

    async fn structure(&self, session: SessionId) -> Result<TmuxStructure> {
        DaemonClient::structure(self, session).await
    }

    async fn tmux_command(&self, session: SessionId, command: &str) -> Result<Vec<String>> {
        DaemonClient::tmux_command(self, session, command).await
    }

    async fn close_tmux(&self, session: SessionId) -> Result<()> {
        DaemonClient::close_tmux(self, session).await
    }

    async fn probe_tmux(&self, session: SessionId) -> bool {
        DaemonClient::probe_tmux(self, session).await
    }

    async fn probe_link(&self, link: LinkId) -> Result<bool> {
        client::probe_link(link).await
    }

    async fn probe_buffer(&self, buffer: BufferId) -> Result<bool> {
        client::probe_buffer(buffer).await
    }

    fn transport(&self, link: LinkId) -> Arc<dyn Transport> {
        Arc::new(DaemonTransport::new(link))
    }
}
