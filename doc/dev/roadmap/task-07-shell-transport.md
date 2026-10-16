# Task 7: ShellTransport (real)

**Objective:** `Transport` impl that spawns a local child with stdin/stdout pipes; `address()` returns `LinkAddress::Shell`.

**Files:**
- Modify: `crates/xscore/src/transport/shell.rs`

```rust
// transport/shell.rs
use crate::core::{Error, LinkAddress, Result};
use async_trait::async_trait;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::{Child, Command};
use tokio::sync::Mutex;

pub struct ShellTransport {
    pub command: String,
    pub args: Vec<String>,
    pub cwd: Option<String>,
    child: Mutex<Option<Child>>,
}

impl ShellTransport {
    pub async fn new(command: String, args: Vec<String>, cwd: Option<String>) -> Result<Self> {
        Ok(Self { command, args, cwd, child: Mutex::new(None) })
    }
}

#[async_trait]
impl super::Transport for ShellTransport {
    fn kind(&self) -> &'static str { "shell" }
    fn address(&self) -> LinkAddress {
        LinkAddress::Shell { command: self.command.clone(), args: self.args.clone() }
    }

    async fn open(&mut self) -> Result<()> {
        let mut cmd = Command::new(&self.command);
        cmd.args(&self.args);
        if let Some(c) = &self.cwd { cmd.current_dir(c); }
        cmd.stdin(std::process::Stdio::piped()).stdout(std::process::Stdio::piped());
        let child = cmd.spawn().map_err(|e| Error::Transport(format!("spawn: {e}")))?;
        *self.child.lock().await = Some(child);
        Ok(())
    }

    async fn write(&self, data: &[u8]) -> Result<usize> {
        let mut g = self.child.lock().await;
        let c = g.as_mut().ok_or_else(|| Error::Transport("not open".into()))?;
        let stdin = c.stdin.as_mut().ok_or_else(|| Error::Transport("stdin closed".into()))?;
        stdin.write_all(data).await?;
        Ok(data.len())
    }

    async fn read(&self, buf: &mut [u8]) -> Result<usize> {
        let mut g = self.child.lock().await;
        let c = g.as_mut().ok_or_else(|| Error::Transport("not open".into()))?;
        let stdout = c.stdout.as_mut().ok_or_else(|| Error::Transport("stdout closed".into()))?;
        Ok(stdout.read(buf).await?)
    }

    async fn resize(&self, _: u16, _: u16) -> Result<()> { Ok(()) }
    async fn close(&self) -> Result<()> {
        let mut g = self.child.lock().await;
        if let Some(c) = g.as_mut() { c.kill().await?; }
        *g = None;
        Ok(())
    }
}
```

**Step 1: Test**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn shell_transport_roundtrip() {
        let mut t = ShellTransport::new("cat".into(), vec![], None).await.unwrap();
        t.open().await.unwrap();
        t.write(b"hi\n").await.unwrap();
        let mut buf = [0u8; 16];
        let n = t.read(&mut buf).await.unwrap();
        assert!(std::str::from_utf8(&buf[..n]).unwrap().contains("hi"));
        t.close().await.unwrap();
    }
}
```

**Step 2: Commit**

```bash
git add crates/xscore/src/transport/shell.rs
git commit -m "feat(transport/shell): real ShellTransport with address()"
```