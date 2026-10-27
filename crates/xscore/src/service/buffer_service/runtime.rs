//! Runtime resources attached to one Buffer.
//!
//! A Buffer is one logical byte owner. Its runtime may be local, daemon-owned,
//! or a tmux control target. Those are backend states of the same Buffer, not
//! separate adapter objects, so one enum performs the dispatch.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::Mutex;

use super::ring::{RingBuffer, DIRECT_BUFFER_CAPACITY};
use super::store::BufferId;
use crate::infra::link::Transport;
use crate::infra::tmux::TmuxControl;
use crate::service::backend::daemon_port::DaemonPort;
use crate::{Error, Result};

const READ_WAIT: Duration = Duration::from_millis(200);
const READ_POLL: Duration = Duration::from_millis(10);

pub(crate) enum BufferRuntime {
    Local(LocalRuntime),
    Daemon(DaemonRuntime),
    Control(ControlRuntime),
}

impl BufferRuntime {
    pub(crate) fn local(transport: Arc<dyn Transport>, buffer: Arc<Mutex<RingBuffer>>) -> Self {
        Self::Local(LocalRuntime::new(transport, buffer))
    }

    pub(crate) fn daemon(client: Arc<dyn DaemonPort>, buffer: BufferId) -> Self {
        Self::Daemon(DaemonRuntime { client, buffer })
    }

    pub(crate) fn control(
        control: Arc<TmuxControl>,
        handle: impl Into<String>,
        buffer: Arc<Mutex<RingBuffer>>,
    ) -> Self {
        Self::Control(ControlRuntime {
            control,
            handle: handle.into(),
            buffer,
        })
    }

    pub(crate) async fn read(&self, out: &mut [u8]) -> Result<usize> {
        match self {
            Self::Local(runtime) => read_ring(&runtime.buffer, out).await,
            Self::Daemon(runtime) => runtime.read(out).await,
            Self::Control(runtime) => read_ring(&runtime.buffer, out).await,
        }
    }

    pub(crate) async fn write(&self, data: &[u8]) -> Result<usize> {
        match self {
            Self::Local(runtime) => runtime.transport.write(data).await,
            Self::Daemon(runtime) => runtime.client.write(runtime.buffer, data).await,
            Self::Control(runtime) => runtime.write(data).await,
        }
    }

    pub(crate) async fn resize(&self, cols: u16, rows: u16) -> Result<()> {
        match self {
            Self::Local(runtime) => runtime.transport.resize(cols, rows).await,
            Self::Daemon(runtime) => runtime.client.resize(runtime.buffer, cols, rows).await,
            Self::Control(runtime) => runtime.resize(cols, rows).await,
        }
    }

    /// Stops resources owned by this Buffer runtime. It never closes a Link
    /// transport or a tmux control client, both of which may be shared.
    pub(crate) async fn close(&self) -> Result<()> {
        match self {
            Self::Local(runtime) => runtime.close().await,
            Self::Daemon(runtime) => runtime.client.detach(runtime.buffer).await,
            Self::Control(_) => Ok(()),
        }
    }
}

pub(crate) struct LocalRuntime {
    transport: Arc<dyn Transport>,
    buffer: Arc<Mutex<RingBuffer>>,
    stop: Arc<AtomicBool>,
    task: Mutex<Option<tokio::task::JoinHandle<()>>>,
}

impl LocalRuntime {
    fn new(transport: Arc<dyn Transport>, buffer: Arc<Mutex<RingBuffer>>) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let task = spawn_reader(
            Arc::clone(&transport),
            Arc::clone(&buffer),
            Arc::clone(&stop),
        );
        Self {
            transport,
            buffer,
            stop,
            task: Mutex::new(Some(task)),
        }
    }

    async fn close(&self) -> Result<()> {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(task) = self.task.lock().await.take() {
            task.abort();
        }
        Ok(())
    }
}

pub(crate) struct DaemonRuntime {
    client: Arc<dyn DaemonPort>,
    buffer: BufferId,
}

impl DaemonRuntime {
    async fn read(&self, out: &mut [u8]) -> Result<usize> {
        let bytes = self.client.read(self.buffer, out.len()).await?;
        let count = bytes.len().min(out.len());
        out[..count].copy_from_slice(&bytes[..count]);
        Ok(count)
    }
}

pub(crate) struct ControlRuntime {
    control: Arc<TmuxControl>,
    handle: String,
    buffer: Arc<Mutex<RingBuffer>>,
}

impl ControlRuntime {
    async fn write(&self, data: &[u8]) -> Result<usize> {
        let payload = std::str::from_utf8(data).map_err(|_| {
            Error::Invalid("a tmux pane takes UTF-8 text; this payload is not valid UTF-8".into())
        })?;
        let command = crate::infra::tmux::send_keys_literal(&self.handle, payload)?;
        self.control.send(&command).await?;
        Ok(data.len())
    }

    async fn resize(&self, cols: u16, rows: u16) -> Result<()> {
        self.control
            .send(&crate::infra::tmux::resize_pane_command(
                &self.handle,
                cols,
                rows,
            ))
            .await?;
        Ok(())
    }
}

fn spawn_reader(
    transport: Arc<dyn Transport>,
    buffer: Arc<Mutex<RingBuffer>>,
    stop: Arc<AtomicBool>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut chunk = [0u8; 4096];
        while !stop.load(Ordering::Relaxed) {
            match tokio::time::timeout(READ_POLL, transport.read(&mut chunk)).await {
                Ok(Ok(0)) => tokio::time::sleep(READ_POLL).await,
                Ok(Ok(count)) => buffer.lock().await.push_slice(&chunk[..count]),
                Ok(Err(_)) => break,
                Err(_) => {}
            }
        }
    })
}

async fn read_ring(buffer: &Mutex<RingBuffer>, out: &mut [u8]) -> Result<usize> {
    let deadline = tokio::time::Instant::now() + READ_WAIT;
    loop {
        let count = buffer.lock().await.copy_out(out);
        if count > 0 {
            return Ok(count);
        }
        if tokio::time::Instant::now() >= deadline {
            return Ok(0);
        }
        tokio::time::sleep(READ_POLL).await;
    }
}

pub(crate) fn fresh_ring() -> Arc<Mutex<RingBuffer>> {
    Arc::new(Mutex::new(RingBuffer::new(DIRECT_BUFFER_CAPACITY)))
}
