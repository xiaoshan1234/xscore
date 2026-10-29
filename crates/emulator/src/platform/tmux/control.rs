//! `tmux -C` 控制客户端：一个 session 一个进程，独立 reader/writer 线程。
//!
//! 跑在独立 PTY 上以隔离宿主控制台（尤其 `wsl.exe` 这类会抢占控制台输入的程序）。
//! `-F` 分隔符用可打印 `|`，tmux 不会转义，因此 tty 与否都不影响解析。

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::JoinHandle;

use portable_pty::{native_pty_system, Child as PtyChild, CommandBuilder, MasterPty, PtySize};
use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender};
use tokio::sync::oneshot;

use super::format::socket_args;
use super::protocol::{Parsed, ProtocolParser, TmuxEvent};
use super::TmuxSocket;
use crate::{Error, Result};

fn gone(session: &str) -> Error {
    Error::Transport(format!("tmux session {session} gone"))
}

type PendingReply = oneshot::Sender<Result<Vec<String>>>;

pub fn argv(prefix: &[String], socket: &TmuxSocket, session: &str) -> Vec<String> {
    let mut argv = prefix.to_vec();
    argv.push("tmux".to_string());
    argv.extend(socket_args(socket));
    argv.extend([
        "-C".to_string(),
        "attach".to_string(),
        "-t".to_string(),
        session.to_string(),
    ]);
    argv
}

struct Shared {
    waiters: Mutex<std::collections::VecDeque<PendingReply>>,
    events: Mutex<Vec<UnboundedSender<TmuxEvent>>>,
    panes: Mutex<HashMap<String, Vec<UnboundedSender<Vec<u8>>>>>,
    closed: AtomicBool,
}

impl Shared {
    fn new() -> Self {
        Self {
            waiters: Mutex::new(std::collections::VecDeque::new()),
            events: Mutex::new(Vec::new()),
            panes: Mutex::new(HashMap::new()),
            closed: AtomicBool::new(false),
        }
    }

    fn deliver_output(&self, pane: &str, bytes: Vec<u8>) {
        let mut panes = lock(&self.panes);
        let Some(subscribers) = panes.get_mut(pane) else {
            return;
        };
        subscribers.retain(|tx| !tx.is_closed());
        for tx in subscribers.iter() {
            let _ = tx.send(bytes.clone());
        }
    }

    fn deliver_event(&self, event: TmuxEvent) {
        let mut events = lock(&self.events);
        events.retain(|tx| !tx.is_closed());
        for tx in events.iter() {
            let _ = tx.send(event.clone());
        }
    }

    fn take_waiter(&self) -> Option<PendingReply> {
        lock(&self.waiters).pop_front()
    }

    fn shutdown(&self) {
        self.closed.store(true, Ordering::SeqCst);
        lock(&self.waiters).clear();
        lock(&self.events).clear();
        lock(&self.panes).clear();
    }
}

pub struct TmuxControl {
    session: String,
    child: Mutex<Option<Box<dyn PtyChild + Send + Sync>>>,
    master: Mutex<Option<Box<dyn MasterPty + Send>>>,
    writer: Mutex<Option<UnboundedSender<String>>>,
    reader: Mutex<Option<JoinHandle<()>>>,
    shared: Arc<Shared>,
}

impl TmuxControl {
    pub fn spawn(prefix: &[String], socket: &TmuxSocket, session: &str) -> Result<TmuxControl> {
        if session.is_empty() {
            return Err(Error::Invalid("tmux session name must not be empty".into()));
        }
        let argv = argv(prefix, socket, session);
        tracing::debug!(argv = %argv.join(" "), session, "tmux control: spawn");
        let (program, args) = argv
            .split_first()
            .ok_or_else(|| Error::Transport("empty tmux command line".into()))?;

        let pair = native_pty_system()
            .openpty(PtySize {
                rows: 40,
                cols: 120,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| Error::Transport(format!("tmux control openpty: {e}")))?;
        let mut command = CommandBuilder::new(program);
        for arg in args {
            command.arg(arg);
        }
        let child = pair
            .slave
            .spawn_command(command)
            .map_err(|e| Error::Transport(format!("spawn `{}`: {e}", argv.join(" "))))?;
        drop(pair.slave);

        let stdout = pair
            .master
            .try_clone_reader()
            .map_err(|e| Error::Transport(format!("tmux control reader: {e}")))?;
        let stdin = pair
            .master
            .take_writer()
            .map_err(|e| Error::Transport(format!("tmux control writer: {e}")))?;

        let shared = Arc::new(Shared::new());
        let (write_tx, write_rx) = mpsc::unbounded_channel::<String>();
        std::thread::Builder::new()
            .name("tmux-control-stdin".into())
            .spawn(move || writer_loop(stdin, write_rx))
            .map_err(|e| Error::Transport(format!("tmux control writer thread: {e}")))?;

        let reader_state = Arc::clone(&shared);
        let reader = std::thread::Builder::new()
            .name("tmux-control-stdout".into())
            .spawn(move || reader_loop(stdout, reader_state))
            .map_err(|e| Error::Transport(format!("tmux control reader thread: {e}")))?;

        Ok(TmuxControl {
            session: session.to_string(),
            child: Mutex::new(Some(child)),
            master: Mutex::new(Some(pair.master)),
            writer: Mutex::new(Some(write_tx)),
            reader: Mutex::new(Some(reader)),
            shared,
        })
    }

    pub async fn send(&self, command: &str) -> Result<Vec<String>> {
        tracing::debug!(command, "tmux control: send");
        if self.shared.closed.load(Ordering::SeqCst) {
            return Err(gone(&self.session));
        }
        let (tx, rx) = oneshot::channel();
        lock(&self.shared.waiters).push_back(tx);

        let queued = match lock(&self.writer).as_ref() {
            Some(writer) => writer.send(format!("{command}\n")).is_ok(),
            None => false,
        };
        if !queued {
            lock(&self.shared.waiters).pop_back();
            return Err(gone(&self.session));
        }

        match rx.await {
            Ok(Ok(reply)) => {
                tracing::debug!(command, lines = reply.len(), "tmux control: reply");
                Ok(reply)
            }
            Ok(Err(e)) => {
                tracing::warn!(command, error = %e, "tmux control: command failed");
                Err(e)
            }
            Err(_) => {
                tracing::warn!(command, "tmux control: reply channel closed");
                Err(gone(&self.session))
            }
        }
    }

    pub fn subscribe(&self) -> UnboundedReceiver<TmuxEvent> {
        let (tx, rx) = mpsc::unbounded_channel();
        if !self.shared.closed.load(Ordering::SeqCst) {
            let mut events = lock(&self.shared.events);
            events.retain(|tx| !tx.is_closed());
            events.push(tx);
        }
        rx
    }

    pub fn output(&self, pane: &str) -> UnboundedReceiver<Vec<u8>> {
        let (tx, rx) = mpsc::unbounded_channel();
        if !self.shared.closed.load(Ordering::SeqCst) {
            let mut panes = lock(&self.shared.panes);
            panes.entry(pane.to_string()).or_default().push(tx);
        }
        rx
    }

    pub fn is_closed(&self) -> bool {
        self.shared.closed.load(Ordering::SeqCst)
    }

    pub async fn close(&self) -> Result<()> {
        self.shared.closed.store(true, Ordering::SeqCst);
        drop(lock(&self.writer).take());
        if let Some(mut child) = lock(&self.child).take() {
            let _ = child.kill();
            let _ = child.wait();
        }
        drop(lock(&self.master).take());
        if let Some(reader) = lock(&self.reader).take() {
            let _ = reader.join();
        }
        self.shared.shutdown();
        Ok(())
    }
}

impl Drop for TmuxControl {
    fn drop(&mut self) {
        drop(lock(&self.writer).take());
        if let Some(mut child) = lock(&self.child).take() {
            let _ = child.kill();
            let _ = child.wait();
        }
        drop(lock(&self.master).take());
    }
}

fn writer_loop<W: Write>(mut stdin: W, mut rx: mpsc::UnboundedReceiver<String>) {
    while let Some(line) = rx.blocking_recv() {
        if stdin
            .write_all(line.as_bytes())
            .and_then(|()| stdin.flush())
            .is_err()
        {
            break;
        }
    }
}

fn reader_loop<R: Read>(stdout: R, shared: Arc<Shared>) {
    let mut parser = ProtocolParser::new();
    let mut open: Option<u64> = None;
    let mut blocks: HashMap<u64, Vec<String>> = HashMap::new();
    let mut claimed: HashMap<u64, PendingReply> = HashMap::new();
    let mut handshake_done = false;
    let mut reader = BufReader::new(stdout);
    let mut line: Vec<u8> = Vec::new();

    loop {
        line.clear();
        match reader.read_until(b'\n', &mut line) {
            Ok(0) => {
                tracing::debug!("tmux control: stdout EOF");
                break;
            }
            Ok(_) => {}
            Err(ref e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => {
                tracing::warn!(error = %e, "tmux control: read error");
                break;
            }
        }

        for parsed in parser.feed(&line) {
            match parsed {
                Parsed::Begin { num, .. } => {
                    tracing::trace!(num, "tmux control: begin");
                    blocks.insert(num, Vec::new());
                    open = Some(num);
                    if handshake_done {
                        if let Some(waiter) = shared.take_waiter() {
                            claimed.insert(num, waiter);
                        }
                    }
                }
                Parsed::End { num } => {
                    tracing::trace!(num, "tmux control: end");
                    let output = blocks.remove(&num).unwrap_or_default();
                    if open == Some(num) {
                        open = None;
                    }
                    if let Some(waiter) = claimed.remove(&num) {
                        let _ = waiter.send(Ok(output));
                    }
                    handshake_done = true;
                }
                Parsed::Error { num, message } => {
                    tracing::warn!(num, message, "tmux control: error");
                    blocks.remove(&num);
                    if open == Some(num) {
                        open = None;
                    }
                    let message = if message.trim().is_empty() {
                        format!("tmux command {num} failed")
                    } else {
                        message
                    };
                    if let Some(waiter) = claimed.remove(&num) {
                        let _ = waiter.send(Err(Error::Transport(message)));
                    }
                    handshake_done = true;
                }
                Parsed::Output { pane, bytes } => {
                    tracing::trace!(pane, len = bytes.len(), "tmux control: output");
                    shared.deliver_output(&pane, bytes);
                }
                Parsed::Event(event) => {
                    let exit = matches!(&event, TmuxEvent::Exit { .. });
                    if exit {
                        if let TmuxEvent::Exit { reason } = &event {
                            tracing::info!(reason, "tmux control: exit");
                        }
                    }
                    shared.deliver_event(event);
                    if exit {
                        shared.shutdown();
                        return;
                    }
                }
                Parsed::Unknown(text) => {
                    if let Some(num) = open {
                        if claimed.contains_key(&num) {
                            if let Some(output) = blocks.get_mut(&num) {
                                output.push(text);
                                continue;
                            }
                        }
                    }
                    if text.starts_with('%') {
                        continue;
                    }
                }
            }
        }
    }

    shared.shutdown();
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn argv_is_the_measured_shape() {
        assert_eq!(
            argv(&[], &TmuxSocket::Default, "work"),
            ["tmux", "-C", "attach", "-t", "work"]
        );
        let prefix: Vec<String> = ["ssh", "-o", "BatchMode=yes", "devbox"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(
            argv(&prefix, &TmuxSocket::Named("x".into()), "work"),
            [
                "ssh",
                "-o",
                "BatchMode=yes",
                "devbox",
                "tmux",
                "-L",
                "x",
                "-C",
                "attach",
                "-t",
                "work"
            ]
        );
    }
}
