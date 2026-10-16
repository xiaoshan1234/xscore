# Task 5: Sink trait with per-pane ops + TmuxSink + RawSink

**Objective:** Define `Sink` trait with `new_pane`/`close_pane`/`list_panes`/`snapshot(pane_handle, ...)`/`write(pane_handle, ...)`/`resize(pane_handle, ...)`. TmuxSink wraps tmux; RawSink stub.

**Files:**
- Create: `crates/xscore/src/sink/mod.rs`
- Create: `crates/xscore/src/sink/tmux.rs`
- Create: `crates/xscore/src/sink/raw.rs`
- Create: `crates/xscore/src/sink/factory.rs`
- Create: `crates/xscore/src/util/mod.rs`
- Create: `crates/xscore/src/util/tmux_socket.rs`

**Step 1: socket helper**

```rust
// util/tmux_socket.rs
pub const DEFAULT_SOCKET: &str = "xscore";
pub fn socket_name() -> String {
    std::env::var("XSCORE_SOCKET").unwrap_or_else(|_| DEFAULT_SOCKET.to_string())
}
pub fn tmux_arg_socket() -> Vec<String> { vec!["-L".into(), socket_name()] }
```

```rust
// util/mod.rs
pub mod tmux_socket;
pub use tmux_socket::{socket_name, tmux_arg_socket};
```

**Step 2: Failing test for Sink**

```rust
// sink/mod.rs (test)
#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use crate::transport::Transport;

    struct Noop;
    #[async_trait]
    impl Sink for Noop {
        fn kind(&self) -> &'static str { "noop" }
        async fn attach(&mut self, _: &dyn Transport) -> Result<()> { Ok(()) }
        async fn new_pane(&self) -> Result<String> { Ok("pane-stub".into()) }
        async fn close_pane(&self, _: &str) -> Result<()> { Ok(()) }
        async fn list_panes(&self) -> Result<Vec<String>> { Ok(vec![]) }
        async fn snapshot(&self, _: &str, _: &mut [u8]) -> Result<usize> { Ok(0) }
        async fn write(&self, _: &str, data: &[u8]) -> Result<usize> { Ok(data.len()) }
        async fn resize(&self, _: &str, _: u16, _: u16) -> Result<()> { Ok(()) }
        async fn close(&self) -> Result<()> { Ok(()) }
    }

    #[tokio::test]
    async fn noop_sink_creates_and_closes_pane() {
        let s = Noop;
        let p = s.new_pane().await.unwrap();
        assert_eq!(p, "pane-stub");
        s.close_pane(&p).await.unwrap();
    }
}
```

**Step 3: Implement Sink trait**

```rust
// sink/mod.rs
use crate::core::Result;
use crate::transport::Transport;
use async_trait::async_trait;

#[async_trait]
pub trait Sink: Send + Sync {
    fn kind(&self) -> &'static str;
    /// Wire this sink to the link's transport. Called once per link lifetime.
    async fn attach(&mut self, transport: &dyn Transport) -> Result<()>;
    /// Open a new pane inside this link. Returns sink-side handle.
    async fn new_pane(&self) -> Result<String>;
    async fn close_pane(&self, pane_handle: &str) -> Result<()>;
    async fn list_panes(&self) -> Result<Vec<String>>;
    async fn snapshot(&self, pane_handle: &str, buf: &mut [u8]) -> Result<usize>;
    async fn write(&self, pane_handle: &str, data: &[u8]) -> Result<usize>;
    async fn resize(&self, pane_handle: &str, cols: u16, rows: u16) -> Result<()>;
    async fn close(&self) -> Result<()>;
}

pub mod factory;
pub mod raw;
pub mod tmux;
```

**Step 4: TmuxSink**

```rust
// sink/tmux.rs
use crate::core::{Error, Result};
use crate::transport::Transport;
use crate::util::tmux_arg_socket;
use async_trait::async_trait;
use tokio::process::Command;

pub struct TmuxSink {
    pub mode: String,             // "local" | "remote" (post-resolution)
    pub session_name: String,     // tmux server session name == the link key
}

impl TmuxSink {
    pub async fn create(mode: &str, session_name: &str, transport: &mut dyn Transport) -> Result<Self> {
        transport.open().await?;
        let mut args = tmux_arg_socket();
        args.extend([
            "new-session".into(), "-d".into(), "-s".into(), session_name.into(),
            "-x".into(), "120".into(), "-y".into(), "30".into(),
        ]);
        let status = Command::new("tmux").args(&args).status().await?;
        if !status.success() {
            return Err(Error::Sink(format!("tmux new-session: exit {status}")));
        }
        Ok(Self { mode: mode.into(), session_name: session_name.into() })
    }

    fn tmux(&self) -> Command {
        let mut c = Command::new("tmux");
        c.args(tmux_arg_socket());
        c
    }
}

#[async_trait]
impl super::Sink for TmuxSink {
    fn kind(&self) -> &'static str { "tmux" }

    async fn attach(&mut self, _transport: &dyn Transport) -> Result<()> {
        // tmux already running on its own socket; attach is reserved for ssh-channel
        // wiring in the SSH-integration PR. No-op for now.
        Ok(())
    }

    async fn new_pane(&self) -> Result<String> {
        // tmux new-window returns pane id like "%42"; we capture stdout
        let out = self.tmux()
            .args(["new-window", "-t", &self.session_name, "-P", "-F", "#{pane_id}"])
            .output().await?;
        if !out.status.success() {
            return Err(Error::Sink(format!("tmux new-window: {:?}", out.status)));
        }
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    }

    async fn close_pane(&self, pane_handle: &str) -> Result<()> {
        let status = self.tmux()
            .args(["kill-pane", "-t", pane_handle])
            .status().await?;
        if !status.success() {
            return Err(Error::Sink(format!("kill-pane exit {status}")));
        }
        Ok(())
    }

    async fn list_panes(&self) -> Result<Vec<String>> {
        let out = self.tmux()
            .args(["list-panes", "-t", &self.session_name, "-F", "#{pane_id}"])
            .output().await?;
        if !out.status.success() {
            return Err(Error::Sink(format!("list-panes exit {:?}", out.status)));
        }
        Ok(out.lines().filter(|l| !l.is_empty()).map(String::from).collect())
    }

    async fn snapshot(&self, pane_handle: &str, buf: &mut [u8]) -> Result<usize> {
        let out = self.tmux()
            .args(["capture-pane", "-p", "-t", pane_handle, "-J"])
            .output().await?;
        if !out.status.success() {
            return Err(Error::Sink(format!("capture-pane exit {:?}", out.status)));
        }
        let bytes = &out.stdout;
        let n = bytes.len().min(buf.len());
        buf[..n].copy_from_slice(&bytes[..n]);
        Ok(n)
    }

    async fn write(&self, pane_handle: &str, data: &[u8]) -> Result<usize> {
        let s = std::str::from_utf8(data).map_err(|e| Error::Invalid(e.to_string()))?;
        let mut cmd = self.tmux();
        cmd.arg("send-keys").arg("-t").arg(pane_handle);
        for ch in s.chars() { cmd.arg(ch.to_string()); }
        let status = cmd.status().await?;
        if !status.success() { return Err(Error::Sink(format!("send-keys exit {status}"))); }
        Ok(data.len())
    }

    async fn resize(&self, pane_handle: &str, cols: u16, rows: u16) -> Result<()> {
        let status = self.tmux()
            .args(["resize-pane", "-t", pane_handle,
                   "-x", &cols.to_string(), "-y", &rows.to_string()])
            .status().await?;
        if !status.success() { return Err(Error::Sink(format!("resize-pane exit {status}"))); }
        Ok(())
    }

    async fn close(&self) -> Result<()> {
        let _ = self.tmux()
            .args(["kill-session", "-t", &self.session_name])
            .status().await?;
        Ok(())
    }
}
```

**Step 5: RawSink** (one pane per link; no-op semantics)

```rust
// sink/raw.rs
use crate::core::Result;
use crate::transport::Transport;
use async_trait::async_trait;

pub struct RawSink;

#[async_trait]
impl super::Sink for RawSink {
    fn kind(&self) -> &'static str { "raw" }
    async fn attach(&mut self, _: &dyn Transport) -> Result<()> { Ok(()) }
    async fn new_pane(&self) -> Result<String> { Ok("raw-pane-0".into()) }
    async fn close_pane(&self, _: &str) -> Result<()> { Ok(()) }
    async fn list_panes(&self) -> Result<Vec<String>> { Ok(vec!["raw-pane-0".into()]) }
    async fn snapshot(&self, _: &str, _: &mut [u8]) -> Result<usize> { Ok(0) }
    async fn write(&self, _: &str, _: &[u8]) -> Result<usize> { Ok(0) }
    async fn resize(&self, _: &str, _: u16, _: u16) -> Result<()> { Ok(()) }
    async fn close(&self) -> Result<()> { Ok(()) }
}
```

**Step 6: factory**

```rust
// sink/factory.rs
use crate::core::Result;
use crate::model::SinkConfig;
use crate::transport::Transport;
use super::{raw::RawSink, tmux::TmuxSink, Sink};

pub async fn sink_for(
    config: &SinkConfig,
    transport_kind: &str,
    transport: &mut dyn Transport,
) -> Result<Box<dyn Sink>> {
    match config {
        SinkConfig::Tmux { mode, session_name } => {
            let resolved_mode = match mode.as_str() {
                "auto" => if transport_kind == "ssh" { "remote" } else { "local" },
                other => other,
            };
            if resolved_mode == "off" {
                return Ok(Box::new(RawSink));
            }
            let name = session_name.clone()
                .unwrap_or_else(|| format!("xscore-{}", uuid::Uuid::now_v7()));
            let s = TmuxSink::create(resolved_mode, &name, transport).await?;
            Ok(Box::new(s))
        }
    }
}
```

**Step 7: Run tests**

Run: `cargo test -p xscore sink:: util::`
Expected: passing.

**Step 8: Commit**

```bash
git add crates/xscore/src/sink/ crates/xscore/src/util/
git commit -m "feat(sink): Sink trait with per-pane ops + TmuxSink (multi-pane) + RawSink"
```