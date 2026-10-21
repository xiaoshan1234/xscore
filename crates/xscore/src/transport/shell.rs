// transport/shell.rs
//
// A shell link is a **pty**, not a pipe (ADR 0010; arch.md §4 "Who owns the
// stream"). The earlier `Stdio::piped()` version made the child block-buffer
// its output, left `isatty` false, and lost echo / job control / `SIGWINCH` —
// so `vim`, `sudo` and `python -i` all took their non-interactive path.
//
// Threading: `portable-pty` is blocking, so each direction gets its own OS
// thread and the async side talks to it over an unbounded channel:
//
//   child ──pty──► reader thread ──unbounded──► read()
//   write() ──unbounded──► writer thread ──pty──► child
//
// That is also what fixes the old read/write starvation: the two directions no
// longer share a lock, so a read awaiting output cannot block a write.

use crate::core::{Error, LinkAddress, Result};
use async_trait::async_trait;
use portable_pty::{native_pty_system, Child, CommandBuilder, MasterPty, PtySize};
use std::collections::VecDeque;
use std::io::{Read, Write};
use std::sync::Mutex as StdMutex;
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender};
use tokio::sync::Mutex;

const DEFAULT_COLS: u16 = 80;
const DEFAULT_ROWS: u16 = 24;
const READ_CHUNK: usize = 8 * 1024;

/// The async end of the pty: a channel of chunks, plus whatever did not fit in
/// the caller's buffer last time.
type PtyOutput = (UnboundedReceiver<Vec<u8>>, VecDeque<u8>);

pub struct ShellTransport {
    pub command: String,
    pub args: Vec<String>,
    pub cwd: Option<String>,
    /// Kept only so `resize` can reach `TIOCSWINSZ`.
    master: StdMutex<Option<Box<dyn MasterPty + Send>>>,
    /// Kept so `close` can kill and reap the child.
    child: StdMutex<Option<Box<dyn Child + Send + Sync>>>,
    /// Bytes headed for the child. Unbounded, so `write` never awaits a lock
    /// that a read might be holding.
    to_child: StdMutex<Option<UnboundedSender<Vec<u8>>>>,
    /// Bytes from the child, plus whatever did not fit in the caller's buffer.
    from_child: Mutex<Option<PtyOutput>>,
}

impl std::fmt::Debug for ShellTransport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ShellTransport")
            .field("command", &self.command)
            .field("args", &self.args)
            .field("cwd", &self.cwd)
            .finish_non_exhaustive()
    }
}

impl ShellTransport {
    pub async fn new(command: String, args: Vec<String>, cwd: Option<String>) -> Result<Self> {
        Ok(Self {
            command,
            args,
            cwd,
            master: StdMutex::new(None),
            child: StdMutex::new(None),
            to_child: StdMutex::new(None),
            from_child: Mutex::new(None),
        })
    }
}

#[async_trait]
impl super::Transport for ShellTransport {
    fn kind(&self) -> &'static str { "shell" }

    fn address(&self) -> LinkAddress {
        LinkAddress::Shell { command: self.command.clone(), args: self.args.clone() }
    }

    async fn open(&mut self) -> Result<()> {
        let already_open = self.child.lock().map(|g| g.is_some()).unwrap_or(false);
        if already_open {
            self.close().await?;
        }

        let pair = native_pty_system()
            .openpty(PtySize {
                rows: DEFAULT_ROWS,
                cols: DEFAULT_COLS,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| Error::Transport(format!("openpty: {e}")))?;

        let mut cmd = CommandBuilder::new(&self.command);
        for arg in &self.args {
            cmd.arg(arg);
        }
        if let Some(cwd) = &self.cwd {
            cmd.cwd(cwd);
        }
        // A tty without TERM is a terminal that cannot address a screen.
        if cmd.get_env("TERM").is_none() {
            cmd.env("TERM", "xterm-256color");
        }

        let child = pair
            .slave
            .spawn_command(cmd)
            .map_err(|e| Error::Transport(format!("spawn: {e}")))?;
        // Drop the slave so the pty reports EOF when the child exits.
        drop(pair.slave);

        let mut reader = pair
            .master
            .try_clone_reader()
            .map_err(|e| Error::Transport(format!("pty reader: {e}")))?;
        let mut writer = pair
            .master
            .take_writer()
            .map_err(|e| Error::Transport(format!("pty writer: {e}")))?;

        let (out_tx, out_rx) = unbounded_channel::<Vec<u8>>();
        std::thread::Builder::new()
            .name("xscore-pty-read".into())
            .spawn(move || {
                let mut buf = vec![0u8; READ_CHUNK];
                loop {
                    match reader.read(&mut buf) {
                        Ok(0) => break,
                        Ok(n) => {
                            if out_tx.send(buf[..n].to_vec()).is_err() {
                                break;
                            }
                        }
                        // EIO on Linux once the child side is gone.
                        Err(_) => break,
                    }
                }
            })
            .map_err(|e| Error::Transport(format!("pty reader thread: {e}")))?;

        let (in_tx, mut in_rx) = unbounded_channel::<Vec<u8>>();
        std::thread::Builder::new()
            .name("xscore-pty-write".into())
            .spawn(move || {
                while let Some(chunk) = in_rx.blocking_recv() {
                    if writer.write_all(&chunk).is_err() {
                        break;
                    }
                    let _ = writer.flush();
                }
            })
            .map_err(|e| Error::Transport(format!("pty writer thread: {e}")))?;

        *self.master.lock().unwrap() = Some(pair.master);
        *self.child.lock().unwrap() = Some(child);
        *self.to_child.lock().unwrap() = Some(in_tx);
        *self.from_child.lock().await = Some((out_rx, VecDeque::new()));
        Ok(())
    }

    async fn write(&self, data: &[u8]) -> Result<usize> {
        if data.is_empty() {
            return Ok(0);
        }
        let tx = self
            .to_child
            .lock()
            .unwrap()
            .clone()
            .ok_or_else(|| Error::Transport("not open".into()))?;
        tx.send(data.to_vec())
            .map_err(|_| Error::Transport("pty writer closed".into()))?;
        Ok(data.len())
    }

    async fn read(&self, buf: &mut [u8]) -> Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        let mut guard = self.from_child.lock().await;
        let (rx, pending) = guard
            .as_mut()
            .ok_or_else(|| Error::Transport("not open".into()))?;

        if pending.is_empty() {
            match rx.recv().await {
                Some(chunk) => pending.extend(chunk),
                // The pty is gone (child exited, or `close` ran).
                None => return Err(Error::Transport("pty closed".into())),
            }
        }

        let n = pending.len().min(buf.len());
        for slot in buf.iter_mut().take(n) {
            *slot = pending.pop_front().expect("checked length");
        }
        Ok(n)
    }

    async fn resize(&self, cols: u16, rows: u16) -> Result<()> {
        let guard = self.master.lock().unwrap();
        let master = guard.as_ref().ok_or_else(|| Error::Transport("not open".into()))?;
        master
            .resize(PtySize { rows, cols, pixel_width: 0, pixel_height: 0 })
            .map_err(|e| Error::Transport(format!("pty resize: {e}")))
    }

    async fn close(&self) -> Result<()> {
        // Stop the writer first: closing the channel ends its thread and drops
        // the duplicated master fd.
        *self.to_child.lock().unwrap() = None;

        let child = self.child.lock().unwrap().take();
        if let Some(mut child) = child {
            let _ = child.kill();
            let _ = tokio::task::spawn_blocking(move || child.wait()).await;
        }
        // Dropping the master makes the reader thread's blocking read return.
        *self.master.lock().unwrap() = None;
        *self.from_child.lock().await = None;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transport::Transport as _;
    use std::time::Duration;

    /// Read until `needle` appears, the pty closes, or we give up.
    async fn read_until(t: &ShellTransport, needle: &str) -> String {
        let mut seen = String::new();
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        while tokio::time::Instant::now() < deadline {
            let mut buf = [0u8; 1024];
            match tokio::time::timeout(Duration::from_millis(200), t.read(&mut buf)).await {
                Ok(Ok(n)) => {
                    seen.push_str(&String::from_utf8_lossy(&buf[..n]));
                    if seen.contains(needle) {
                        break;
                    }
                }
                Ok(Err(_)) => break,
                Err(_) => {}
            }
        }
        seen
    }

    #[tokio::test]
    async fn shell_link_gets_a_tty() {
        let mut t = ShellTransport::new(
            "sh".into(),
            vec!["-c".into(), "test -t 1 && echo IS_TTY || echo NOT_A_TTY".into()],
            None,
        )
        .await
        .unwrap();
        t.open().await.unwrap();
        let out = read_until(&t, "TTY").await;
        assert!(out.contains("IS_TTY"), "child must see a tty, got {out:?}");
        assert!(!out.contains("NOT_A_TTY"), "got {out:?}");
        t.close().await.unwrap();
    }

    #[tokio::test]
    async fn shell_transport_roundtrip() {
        let mut t = ShellTransport::new("cat".into(), vec![], None).await.unwrap();
        t.open().await.unwrap();
        t.write(b"hi\n").await.unwrap();
        let out = read_until(&t, "hi").await;
        assert!(out.contains("hi"), "expected the echo/output, got {out:?}");
        t.close().await.unwrap();
    }

    #[tokio::test]
    async fn resize_reaches_the_pty() {
        let mut t = ShellTransport::new(
            "sh".into(),
            vec!["-c".into(), "sleep 0.4; stty size".into()],
            None,
        )
        .await
        .unwrap();
        t.open().await.unwrap();
        t.resize(100, 40).await.unwrap();
        let out = read_until(&t, "40 100").await;
        assert!(out.contains("40 100"), "child must see the resize, got {out:?}");
        t.close().await.unwrap();
    }

    #[tokio::test]
    async fn write_after_close_fails() {
        let mut t = ShellTransport::new("cat".into(), vec![], None).await.unwrap();
        t.open().await.unwrap();
        t.close().await.unwrap();
        assert!(t.write(b"x").await.is_err());
    }
}
