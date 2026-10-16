# Task 8: SerialTransport (stub) + SshTransport (stub)

Same shape as Task 7 but with `address()` returning the corresponding `LinkAddress` variant.

**Files:**
- Modify: `crates/xscore/src/transport/serial.rs`
- Modify: `crates/xscore/src/transport/ssh.rs`

```rust
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
```

```rust
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
```

**Step 1: Compile + commit**

```bash
cargo build -p xscore
git add crates/xscore/src/transport/serial.rs crates/xscore/src/transport/ssh.rs
git commit -m "feat(transport): Serial/Ssh stubs with address()"
```