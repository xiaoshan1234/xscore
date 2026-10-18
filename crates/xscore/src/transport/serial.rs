// transport/serial.rs
use crate::core::{Error, LinkAddress, Result};
use async_trait::async_trait;

pub struct SerialTransport { pub path: String, pub baud: u32, pub data_bits: u8, pub parity: String, pub stop_bits: u8 }

impl SerialTransport {
    pub async fn new(path: String, baud: u32, data_bits: u8, parity: String, stop_bits: u8) -> Result<Self> {
        Ok(Self { path, baud, data_bits, parity, stop_bits })
    }
}

#[async_trait]
impl super::Transport for SerialTransport {
    fn kind(&self) -> &'static str { "serial" }
    fn address(&self) -> LinkAddress { LinkAddress::Serial { path: self.path.clone() } }
    async fn open(&mut self) -> Result<()> { Err(Error::Transport("SerialTransport::open pending integration phase".into())) }
    async fn write(&self, _: &[u8]) -> Result<usize> { Err(Error::Transport("not open".into())) }
    async fn read(&self, _: &mut [u8]) -> Result<usize> { Err(Error::Transport("not open".into())) }
    async fn resize(&self, _: u16, _: u16) -> Result<()> { Ok(()) }
    async fn close(&self) -> Result<()> { Ok(()) }
}
