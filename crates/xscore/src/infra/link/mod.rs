use crate::domain::{Error, LinkAddress, Result};
use async_trait::async_trait;

#[async_trait]
pub trait Transport: Send + Sync + std::fmt::Debug {
    fn kind(&self) -> &'static str;
    fn address(&self) -> LinkAddress;
    async fn open(&mut self) -> Result<()>;
    async fn write(&self, data: &[u8]) -> Result<usize>;
    async fn read(&self, buf: &mut [u8]) -> Result<usize>;
    async fn resize(&self, cols: u16, rows: u16) -> Result<()>;
    async fn close(&self) -> Result<()>;
}

pub mod factory;
pub mod launcher;
pub mod pty;
pub mod serial;
pub mod ssh;

/// A no-op `Transport` used during state hydration (task-15) for
/// links re-loaded from disk. The link's tmux sink is the runtime
/// source of truth for the sink side; the transport side is moot
/// (the user can re-run `xscore link add <url>` to get a fresh
/// transport for I/O). `link list` / `link del` work; `data read` /
/// `data write` / `data resize` will fail (no I/O) — the user must
/// re-add the link for full functionality.
#[derive(Debug, Default)]
pub struct NoopTransport;

#[async_trait]
impl Transport for NoopTransport {
    fn kind(&self) -> &'static str {
        "noop"
    }
    fn address(&self) -> LinkAddress {
        LinkAddress::Shell {
            command: String::new(),
            args: vec![],
        }
    }
    async fn open(&mut self) -> Result<()> {
        Ok(())
    }
    async fn write(&self, _: &[u8]) -> Result<usize> {
        Ok(0)
    }
    async fn read(&self, _: &mut [u8]) -> Result<usize> {
        Ok(0)
    }
    async fn resize(&self, _: u16, _: u16) -> Result<()> {
        Err(Error::ResizeUnsupported(self.kind()))
    }
    async fn close(&self) -> Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;

    #[derive(Debug)]
    struct Echo;
    #[async_trait]
    impl Transport for Echo {
        fn kind(&self) -> &'static str {
            "echo"
        }
        fn address(&self) -> LinkAddress {
            LinkAddress::Shell {
                command: "echo".into(),
                args: vec![],
            }
        }
        async fn open(&mut self) -> Result<()> {
            Ok(())
        }
        async fn write(&self, data: &[u8]) -> Result<usize> {
            Ok(data.len())
        }
        async fn read(&self, _buf: &mut [u8]) -> Result<usize> {
            Ok(0)
        }
        async fn resize(&self, _: u16, _: u16) -> Result<()> {
            Ok(())
        }
        async fn close(&self) -> Result<()> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn echo_transport_roundtrip() {
        let mut t = Echo;
        t.open().await.unwrap();
        assert_eq!(t.write(b"hi").await.unwrap(), 2);
        t.close().await.unwrap();
    }
}
