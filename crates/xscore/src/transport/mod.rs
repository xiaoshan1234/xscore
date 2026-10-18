use crate::core::{LinkAddress, Result};
use async_trait::async_trait;

#[async_trait]
pub trait Transport: Send + Sync {
    fn kind(&self) -> &'static str;
    fn address(&self) -> LinkAddress;
    async fn open(&mut self) -> Result<()>;
    async fn write(&self, data: &[u8]) -> Result<usize>;
    async fn read(&self, buf: &mut [u8]) -> Result<usize>;
    async fn resize(&self, cols: u16, rows: u16) -> Result<()>;
    async fn close(&self) -> Result<()>;
}

pub mod factory;
pub mod serial;
pub mod shell;
pub mod ssh;

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;

    struct Echo;
    #[async_trait]
    impl Transport for Echo {
        fn kind(&self) -> &'static str { "echo" }
        fn address(&self) -> LinkAddress { LinkAddress::Shell { command: "echo".into(), args: vec![] } }
        async fn open(&mut self) -> Result<()> { Ok(()) }
        async fn write(&self, data: &[u8]) -> Result<usize> { Ok(data.len()) }
        async fn read(&self, _buf: &mut [u8]) -> Result<usize> { Ok(0) }
        async fn resize(&self, _: u16, _: u16) -> Result<()> { Ok(()) }
        async fn close(&self) -> Result<()> { Ok(()) }
    }

    #[tokio::test]
    async fn echo_transport_roundtrip() {
        let mut t = Echo;
        t.open().await.unwrap();
        assert_eq!(t.write(b"hi").await.unwrap(), 2);
        t.close().await.unwrap();
    }
}
