// tmux/control.rs — the control client process (ADR 0010 §D2, arch.md §6).
//
// One process per managed session, and it is the session's single link:
//
//   [prefix...] tmux [-L <name> | -S <path>] -CC attach -t <session>
//
// `prefix` is empty locally and `["ssh", "-o", "BatchMode=yes", ..., <dest>]`
// remotely, so remote is one extra segment in the same argv, not a second
// mechanism (arch.md §6). It is always a freshly spawned process: a control
// protocol sharing a stream with a prompt or echo is the `ThroughTransport`
// mistake again.
//
// Moving parts:
//   * reader thread — owns stdout, feeds `ProtocolParser`, routes;
//   * writer thread — owns stdin. `write_all` on a pipe can block, and blocking
//     the async runtime to send one line is not acceptable, so writes are
//     queued and the thread does the blocking work;
//   * this handle — `send` (correlated by `num`), `subscribe` (events),
//     `output` (one pane's bytes).
//
// stderr is inherited on purpose: the protocol only ever travels on stdout, and
// tmux/ssh diagnostics ("no such session", "Permission denied") belong on the
// user's terminal. Capturing them would need a third thread and a buffer that
// nothing in the protocol layer could act on — failure *semantics*
// (`TmuxTargetGone` vs `TmuxUnreachable`, arch.md §6) are decided one layer up.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::JoinHandle;

use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender};
use tokio::sync::oneshot;

use crate::core::{Error, Result};
use crate::model::TmuxSocket;

use super::protocol::{Parsed, ProtocolParser, TmuxEvent};

/// The one error `send` reports once the client is gone. Also what a `close`
/// makes every in-flight and later command see.
const CLOSED: &str = "control client closed";

type PendingReply = oneshot::Sender<Result<Vec<String>>>;

/// The argv that starts a control client. `prefix` is the transport segment
/// (`[]` locally, `["ssh", ...]` remotely) and `socket` picks the server.
pub fn argv(prefix: &[String], socket: &TmuxSocket, session: &str) -> Vec<String> {
    let mut argv = prefix.to_vec();
    argv.push("tmux".to_string());
    argv.extend(socket_args(socket));
    // `-CC` rather than `-C`: they differ only in disabling terminal echo,
    // which a pipe-based client does not have, and `-CC` is the form people
    // recognise from iTerm2 (ADR 0010 §D2).
    argv.extend(["-CC".to_string(), "attach".to_string(), "-t".to_string(), session.to_string()]);
    argv
}

/// Which tmux server on that host. A named socket resolves *on the host*, so a
/// name is not a path — that is why `TmuxSocket` is not `Option<PathBuf>`
/// (model/session.rs). Kept in one place so local and remote can never drift.
pub fn socket_args(socket: &TmuxSocket) -> Vec<String> {
    match socket {
        TmuxSocket::Default => Vec::new(),
        TmuxSocket::Named(name) => vec!["-L".to_string(), name.clone()],
        TmuxSocket::Path(path) => vec!["-S".to_string(), path.clone()],
    }
}

/// Shared between this handle and the reader thread.
struct Shared {
    /// Command `num` → the waiter for its `%begin…%end` block.
    pending: Mutex<HashMap<u64, PendingReply>>,
    events: Mutex<Vec<UnboundedSender<TmuxEvent>>>,
    /// Pane handle (`"%3"`) → subscribers. Normally one per pane (ADR 0010
    /// §D3); a `Vec` keeps a re-subscribe from silently stealing the channel.
    panes: Mutex<HashMap<String, Vec<UnboundedSender<Vec<u8>>>>>,
    closed: AtomicBool,
}

impl Shared {
    fn new() -> Self {
        Self {
            pending: Mutex::new(HashMap::new()),
            events: Mutex::new(Vec::new()),
            panes: Mutex::new(HashMap::new()),
            closed: AtomicBool::new(false),
        }
    }

    /// Pane bytes go to that pane's subscribers and nowhere else. Bytes for a
    /// pane nobody subscribed to are dropped: tmux does not replay anything, so
    /// there is nothing to catch up on (ADR 0010 §D2), and the buffer that
    /// makes them durable belongs to the pane, not to the connection.
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

    /// Resolve one command's wait, by `<num>` — never by order (ADR 0010 §D2).
    fn finish(&self, num: u64, reply: Result<Vec<String>>) {
        if let Some(tx) = lock(&self.pending).remove(&num) {
            let _ = tx.send(reply);
        }
    }

    /// The client is gone: fail every waiter and end every subscription, so no
    /// caller is left awaiting a reply that can never arrive.
    fn shutdown(&self) {
        self.closed.store(true, Ordering::SeqCst);
        lock(&self.pending).clear();
        lock(&self.events).clear();
        lock(&self.panes).clear();
    }
}

/// A live `tmux -CC` client for one session.
pub struct TmuxControl {
    /// Kept for `kill`/`wait`. Locked only for a syscall, never across an await.
    child: Mutex<Child>,
    /// `None` after `close`: the writer thread then drains and drops the
    /// child's stdin, which is what makes tmux detach.
    writer: Mutex<Option<UnboundedSender<String>>>,
    reader: Mutex<Option<JoinHandle<()>>>,
    shared: Arc<Shared>,
    /// Command numbers start at 1 and only need to be unique per client.
    next_num: AtomicU64,
}

impl TmuxControl {
    /// Spawn a control client. `prefix` is the argv in front of `tmux` (`[]`
    /// locally, `["ssh", ...]` remotely).
    pub fn spawn(prefix: &[String], socket: &TmuxSocket, session: &str) -> Result<TmuxControl> {
        Self::spawn_with(prefix, socket, session, |_| {})
    }

    /// `spawn` plus a hook to adjust the child's `Command` before it starts.
    /// Exists so a test can put a fake `tmux` on the *child's* `PATH` instead of
    /// mutating the test process's environment.
    fn spawn_with(
        prefix: &[String],
        socket: &TmuxSocket,
        session: &str,
        prepare: impl FnOnce(&mut Command),
    ) -> Result<TmuxControl> {
        if session.is_empty() {
            return Err(Error::Invalid("tmux session name must not be empty".into()));
        }
        let argv = argv(prefix, socket, session);
        let (program, args) = argv
            .split_first()
            .ok_or_else(|| Error::Sink("empty tmux command line".into()))?;

        let mut command = Command::new(program);
        command
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit());
        prepare(&mut command);

        let mut child = command
            .spawn()
            .map_err(|e| Error::Sink(format!("spawn `{}`: {e}", argv.join(" "))))?;

        // Both pipes must exist or nothing works; take them before any thread
        // starts, and never leave a half-started child behind on failure.
        let pipes = (child.stdout.take(), child.stdin.take());
        let (stdout, stdin) = match pipes {
            (Some(stdout), Some(stdin)) => (stdout, stdin),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(Error::Sink("tmux control: stdin/stdout pipes unavailable".into()));
            }
        };

        let shared = Arc::new(Shared::new());

        let (write_tx, write_rx) = mpsc::unbounded_channel::<String>();
        std::thread::Builder::new()
            .name("tmux-control-stdin".into())
            .spawn(move || writer_loop(stdin, write_rx))
            .map_err(|e| Error::Sink(format!("tmux control: writer thread: {e}")))?;

        let reader_state = Arc::clone(&shared);
        let reader = std::thread::Builder::new()
            .name("tmux-control-stdout".into())
            .spawn(move || reader_loop(stdout, reader_state))
            .map_err(|e| Error::Sink(format!("tmux control: reader thread: {e}")))?;

        Ok(TmuxControl {
            child: Mutex::new(child),
            writer: Mutex::new(Some(write_tx)),
            reader: Mutex::new(Some(reader)),
            shared,
            next_num: AtomicU64::new(1),
        })
    }

    /// Send one command and return the lines of its `%begin…%end` block.
    pub async fn send(&self, command: &str) -> Result<Vec<String>> {
        if self.shared.closed.load(Ordering::SeqCst) {
            return Err(Error::Sink(CLOSED.into()));
        }
        let num = self.next_num.fetch_add(1, Ordering::SeqCst);
        let (tx, rx) = oneshot::channel();
        // Register *before* writing: the reply can be read before `send` gets
        // back anything, and there is no ordering between the two.
        lock(&self.shared.pending).insert(num, tx);

        let queued = match lock(&self.writer).as_ref() {
            Some(writer) => writer.send(format!("{command}\n")).is_ok(),
            None => false,
        };
        if !queued {
            lock(&self.shared.pending).remove(&num);
            return Err(Error::Sink(CLOSED.into()));
        }

        match rx.await {
            Ok(reply) => reply,
            // The sender was dropped without replying: the reader hit EOF or
            // `%exit`, or `close` cleared the map.
            Err(_) => Err(Error::Sink(CLOSED.into())),
        }
    }

    /// Structural events only. Pane bytes are never events (ADR 0010 §D4).
    pub fn subscribe(&self) -> UnboundedReceiver<TmuxEvent> {
        let (tx, rx) = mpsc::unbounded_channel();
        if !self.shared.closed.load(Ordering::SeqCst) {
            let mut events = lock(&self.shared.events);
            events.retain(|tx| !tx.is_closed());
            events.push(tx);
        }
        // If the client is already gone the sender is dropped here and `rx`
        // yields `None` immediately — a subscriber learns "nothing more".
        rx
    }

    /// Pane output for one handle (`"%3"`), as it arrives.
    pub async fn output(&self, pane: &str) -> Result<UnboundedReceiver<Vec<u8>>> {
        let (tx, rx) = mpsc::unbounded_channel();
        if !self.shared.closed.load(Ordering::SeqCst) {
            let mut panes = lock(&self.shared.panes);
            panes.entry(pane.to_string()).or_default().push(tx);
        }
        Ok(rx)
    }

    /// Whether the client has exited, been closed, or seen `%exit`.
    pub fn is_closed(&self) -> bool {
        self.shared.closed.load(Ordering::SeqCst)
    }

    /// Drop stdin (tmux sees EOF and detaches), kill the child, join the reader.
    pub async fn close(&self) -> Result<()> {
        self.shared.closed.store(true, Ordering::SeqCst);

        // Dropping the writer's sender ends `writer_loop`, which drops the
        // child's stdin: tmux detaches on EOF before it is ever signalled.
        drop(lock(&self.writer).take());

        {
            let mut child = lock(&self.child);
            // Already-exited children reject `kill`; that is not an error here.
            let _ = child.kill();
            let _ = child.wait();
        }

        if let Some(reader) = lock(&self.reader).take() {
            // The child is reaped, so its stdout is at EOF and the reader is on
            // its way out; joining it cannot block on anything else.
            let _ = reader.join();
        }

        self.shared.shutdown();
        Ok(())
    }
}

impl Drop for TmuxControl {
    /// A control client never outlives its handle: a stray `tmux -CC` would keep
    /// a client attached to the user's session with no reader.
    fn drop(&mut self) {
        drop(lock(&self.writer).take());
        let mut child = lock(&self.child);
        let _ = child.kill();
        let _ = child.wait();
    }
}

fn writer_loop(mut stdin: ChildStdin, mut rx: mpsc::UnboundedReceiver<String>) {
    while let Some(line) = rx.blocking_recv() {
        // A write error means the pipe is gone; stop here and let the reader
        // thread's EOF shut the client down.
        if stdin.write_all(line.as_bytes()).and_then(|()| stdin.flush()).is_err() {
            break;
        }
    }
    // `stdin` drops here: EOF to tmux.
}

/// Owns stdout. Every line goes through the parser; routing is by prefix, and
/// events are dispatched the moment they are seen, even inside a block
/// (arch.md §7).
fn reader_loop(stdout: ChildStdout, shared: Arc<Shared>) {
    let mut parser = ProtocolParser::new();
    // Which block is open, and the output collected for each open block. Only
    // this thread touches them, so they need no lock.
    let mut open: Option<u64> = None;
    let mut blocks: HashMap<u64, Vec<String>> = HashMap::new();
    let mut reader = BufReader::new(stdout);
    let mut line: Vec<u8> = Vec::new();

    loop {
        line.clear();
        match reader.read_until(b'\n', &mut line) {
            Ok(0) => break, // EOF: the child is gone
            Ok(_) => {}
            Err(ref e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => break,
        }

        // A read can split a line, so `feed` — not this loop — owns the
        // carry-over. It returns whole records only.
        for parsed in parser.feed(&line) {
            match parsed {
                Parsed::Begin { num, .. } => {
                    blocks.insert(num, Vec::new());
                    open = Some(num);
                }
                Parsed::End { num } => {
                    let output = blocks.remove(&num).unwrap_or_default();
                    if open == Some(num) {
                        open = None;
                    }
                    shared.finish(num, Ok(output));
                }
                Parsed::Error { num, message } => {
                    blocks.remove(&num);
                    if open == Some(num) {
                        open = None;
                    }
                    let message = if message.trim().is_empty() {
                        format!("tmux command {num} failed")
                    } else {
                        message
                    };
                    shared.finish(num, Err(Error::Sink(message)));
                }
                Parsed::Output { pane, bytes } => shared.deliver_output(&pane, bytes),
                Parsed::Event(event) => {
                    let exit = matches!(event, TmuxEvent::Exit { .. });
                    shared.deliver_event(event);
                    if exit {
                        shared.shutdown();
                        return;
                    }
                }
                // A `%` line we do not know is a notification from a tmux that
                // renamed things (ADR 0010 §D4) — drop it, never treat it as a
                // reply. Anything else is the open block's output.
                Parsed::Unknown(text) => {
                    if text.starts_with('%') {
                        continue;
                    }
                    if let Some(num) = open {
                        if let Some(output) = blocks.get_mut(&num) {
                            output.push(text);
                        }
                    }
                }
            }
        }
    }

    shared.shutdown();
}

/// A `std::sync::Mutex` guard, recovering from poisoning. A panic elsewhere must
/// not turn every later command into a panic of its own.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn socket_args_map_one_to_one() {
        assert_eq!(socket_args(&TmuxSocket::Default), Vec::<String>::new());
        assert_eq!(
            socket_args(&TmuxSocket::Named("xscore".into())),
            vec!["-L".to_string(), "xscore".to_string()]
        );
        assert_eq!(
            socket_args(&TmuxSocket::Path("/tmp/tmux-dev".into())),
            vec!["-S".to_string(), "/tmp/tmux-dev".to_string()]
        );
    }

    #[test]
    fn argv_is_the_measured_shape() {
        let local = argv(&[], &TmuxSocket::Default, "work");
        assert_eq!(local, ["tmux", "-CC", "attach", "-t", "work"]);

        // Remote adds one segment in front; nothing else changes (arch.md §6).
        let prefix: Vec<String> =
            ["ssh", "-o", "BatchMode=yes", "devbox"].iter().map(|s| s.to_string()).collect();
        let remote = argv(&prefix, &TmuxSocket::Named("xscore".into()), "work");
        assert_eq!(
            remote,
            [
                "ssh", "-o", "BatchMode=yes", "devbox", "tmux", "-L", "xscore", "-CC", "attach",
                "-t", "work"
            ]
        );
    }

    /// A fake `tmux` that answers the first command with one framed block, with
    /// a pane write interleaved inside it, then idles until stdin closes.
    const FAKE_TMUX: &str = r#"#!/bin/sh
# Fake `tmux -CC` for the protocol tests. `read` first so the reply cannot
# arrive before the client has registered its waiter (real tmux answers a
# command, it does not greet).
read -r _line
printf '%%begin 1700000000 1 0\r\n'
printf 'window 0\r\n'
printf '%%output %%0 tick\\012\r\n'
printf '%%end 1700000000 1 0\r\n'
while read -r _more; do :; done
"#;

    #[cfg(unix)]
    #[tokio::test]
    async fn send_correlates_by_num() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().expect("tempdir");
        let fake = dir.path().join("tmux");
        std::fs::write(&fake, FAKE_TMUX).expect("write fake tmux");
        std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).expect("chmod");
        // PATH is set on the child only; the test process's environment is
        // untouched, so this test can run in parallel with everything else.
        let path = format!(
            "{}:{}",
            dir.path().display(),
            std::env::var("PATH").unwrap_or_default()
        );

        let control = TmuxControl::spawn_with(&[], &TmuxSocket::Default, "probe", |cmd| {
            cmd.env("PATH", &path);
        })
        .expect("spawn fake tmux");

        // Subscribe before sending: output is not replayed to late subscribers.
        let mut panes = control.output("%0").await.expect("pane subscriber");
        let mut events = control.subscribe();

        let reply = control.send("display -p ok").await.expect("send");
        assert_eq!(reply, vec!["window 0".to_string()]);

        // The interleaved `%output` went to the pane, not into the reply.
        let bytes = tokio::time::timeout(std::time::Duration::from_secs(10), panes.recv())
            .await
            .expect("pane output timed out")
            .expect("pane channel closed early");
        assert_eq!(bytes, b"tick\n");

        // ...and pane bytes are never events.
        assert!(events.try_recv().is_err(), "pane bytes leaked into the event stream");

        control.close().await.expect("close");

        // After close, a command must fail loudly instead of hanging forever.
        match control.send("display -p again").await {
            Err(Error::Sink(message)) => assert_eq!(message, CLOSED),
            other => panic!("expected `{CLOSED}`, got {other:?}"),
        }

        // Closing twice is not an error (the child is already reaped).
        control.close().await.expect("second close");
    }
}
