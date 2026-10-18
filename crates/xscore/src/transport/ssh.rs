// transport/ssh.rs
use crate::core::{Error, LinkAddress, Result};
use async_trait::async_trait;

pub struct SshTransport {
    pub host: String, pub port: u16, pub user: String,
    pub identity: Option<String>, pub proxy: Option<String>,
}

impl SshTransport {
    pub async fn connect(host: String, port: u16, user: String, identity: Option<String>, proxy: Option<String>) -> Result<Self> {
        Ok(Self { host, port, user, identity, proxy })
    }
}

#[async_trait]
impl super::Transport for SshTransport {
    fn kind(&self) -> &'static str { "ssh" }
    fn address(&self) -> LinkAddress {
        LinkAddress::Ssh {
            host: self.host.clone(), port: self.port, user: self.user.clone(),
            identity: self.identity.clone(), proxy: self.proxy.clone(),
        }
    }
    async fn open(&mut self) -> Result<()> { Err(Error::Transport("SshTransport::open pending integration phase".into())) }
    async fn write(&self, _: &[u8]) -> Result<usize> { Err(Error::Transport("not open".into())) }
    async fn read(&self, _: &mut [u8]) -> Result<usize> { Err(Error::Transport("not open".into())) }
    async fn resize(&self, _: u16, _: u16) -> Result<()> { Ok(()) }
    async fn close(&self) -> Result<()> { Ok(()) }
}
