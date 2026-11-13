//! `tmux -C` 控制客户端：一个 session 一个传输，统一处理本地 PTY 与 SSH channel。
//!
//! Local/WSL 跑在独立 PTY 上以隔离宿主控制台；SSH 通过 russh exec channel 传输。
//! `-F` 分隔符用可打印 `|`，tmux 不会转义，因此 tty 与否都不影响解析。

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use async_trait::async_trait;
use portable_pty::{native_pty_system, Child as PtyChild, CommandBuilder, MasterPty, PtySize};
use russh::Disconnect;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader as TokioBufReader};
use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender};
use tokio::sync::oneshot;
use tokio::task::JoinHandle;

use super::format::{process_prefix, quote_arg, socket_args};
use super::protocol::{Parsed, ProtocolParser, TmuxEvent};
use super::{TmuxHostConfig, TmuxSocket};
use crate::platform::ssh::{connect_authenticated, SshClient};
use crate::{Error, Result};

fn gone(session: &str) -> Error {
    Error::Transport(format!("tmux session {session} gone"))
}

type PendingReply = oneshot::Sender<Result<Vec<String>>>;
type ReaderTask = JoinHandle<()>;

/// 构造 `tmux -C attach` 的完整命令行。
///
/// 参数/输出：`prefix`、`socket` 和 `session` 为输入；返回按执行顺序排列的参数列表。
/// How [`TmuxControl::spawn`] attaches to (or creates) the tmux session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TmuxSpawnMode {
    /// Attach to an existing session (`tmux -C attach -t <session>`).
    Attach,
    /// Create a new session and bind the control client to it
    /// (`tmux -C new-session -A -s <session> -x <cols> -y <rows>`).
    NewSession {
        /// Initial terminal columns.
        cols: u16,
        /// Initial terminal rows.
        rows: u16,
    },
}

fn argv_for_mode(
    prefix: &[String],
    socket: &TmuxSocket,
    session: &str,
    mode: TmuxSpawnMode,
) -> Vec<String> {
    let mut argv = prefix.to_vec();
    argv.push("tmux".to_string());
    argv.extend(socket_args(socket));
    argv.push("-C".to_string());
    match mode {
        TmuxSpawnMode::Attach => {
            argv.extend(["attach".to_string(), "-t".to_string(), session.to_string()])
        }
        TmuxSpawnMode::NewSession { cols, rows } => argv.extend([
            "new-session".to_string(),
            "-A".to_string(),
            "-s".to_string(),
            session.to_string(),
            "-x".to_string(),
            cols.to_string(),
            "-y".to_string(),
            rows.to_string(),
        ]),
    }
    argv
}

/// Build the complete `tmux -C attach` command line.
pub fn argv(prefix: &[String], socket: &TmuxSocket, session: &str) -> Vec<String> {
    argv_for_mode(prefix, socket, session, TmuxSpawnMode::Attach)
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

type ChildHandle = Box<dyn PtyChild + Send + Sync>;

fn kill_child(child: &mut ChildHandle) {
    let _ = child.kill();
    let _ = child.wait();
}

/// tmux 控制命令的底层写入与关闭接口。
#[async_trait]
trait TmuxTransport: Send + Sync {
    /// 向 tmux 控制连接写入一条不含换行的命令。
    ///
    /// 参数/输出：`line` 为输入，实现在写入时补充换行；成功返回空值。
    async fn write_command(&self, line: &str) -> Result<()>;

    /// 关闭 tmux 控制连接及其底层资源。
    ///
    /// 参数/输出：`self` 为输入输出；成功关闭时返回空值。
    async fn close(&self) -> Result<()>;
}

fn queue_line(writer: &Mutex<Option<UnboundedSender<String>>>, line: &str) -> Result<()> {
    let queued = match lock(writer).as_ref() {
        Some(writer) => writer.send(format!("{line}\n")).is_ok(),
        None => false,
    };
    if queued {
        Ok(())
    } else {
        Err(Error::Transport("tmux control transport closed".into()))
    }
}

struct LocalTransport {
    child: Mutex<Option<ChildHandle>>,
    master: Mutex<Option<Box<dyn MasterPty + Send>>>,
    writer: Mutex<Option<UnboundedSender<String>>>,
}

#[async_trait]
impl TmuxTransport for LocalTransport {
    async fn write_command(&self, line: &str) -> Result<()> {
        queue_line(&self.writer, line)
    }

    async fn close(&self) -> Result<()> {
        drop(lock(&self.writer).take());
        let child = lock(&self.child).take();
        if let Some(mut child) = child {
            let _ = tokio::task::spawn_blocking(move || kill_child(&mut child)).await;
        }
        drop(lock(&self.master).take());
        Ok(())
    }
}

impl Drop for LocalTransport {
    fn drop(&mut self) {
        drop(lock(&self.writer).take());
        if let Some(mut child) = lock(&self.child).take() {
            kill_child(&mut child);
        }
        drop(lock(&self.master).take());
    }
}

struct SshTransport {
    handle: Mutex<Option<SshClient>>,
    writer: Mutex<Option<UnboundedSender<String>>>,
}

#[async_trait]
impl TmuxTransport for SshTransport {
    async fn write_command(&self, line: &str) -> Result<()> {
        queue_line(&self.writer, line)
    }

    async fn close(&self) -> Result<()> {
        drop(lock(&self.writer).take());
        let Some(client) = lock(&self.handle).take() else {
            return Ok(());
        };
        client
            .handle
            .disconnect(Disconnect::ByApplication, "", "")
            .await
            .map_err(|e| Error::Transport(format!("ssh disconnect: {e}")))
    }
}

/// 一个 tmux session 的控制模式客户端。
///
/// 持有传输后端、读取任务、命令回复队列、事件与 pane 输出订阅。
pub struct TmuxControl {
    session: String,
    transport: Arc<dyn TmuxTransport>,
    reader: Mutex<Option<ReaderTask>>,
    shared: Arc<Shared>,
    command_lock: tokio::sync::Mutex<()>,
}

impl TmuxControl {
    /// 返回当前控制连接对应的 tmux session 名。
    ///
    /// 参数/输出：`self` 为输入；返回 session 名引用。
    pub fn session(&self) -> &str {
        &self.session
    }

    /// 启动 tmux 控制传输并建立输出读取任务。
    ///
    /// 参数/输出：`host`、`socket` 和 `session` 为输入；本地/WSL 使用 PTY 子进程，
    /// SSH 使用 russh exec channel；返回已启动的 [`TmuxControl`]。
    pub async fn spawn(
        host: &TmuxHostConfig,
        socket: &TmuxSocket,
        session: &str,
    ) -> Result<TmuxControl> {
        Self::spawn_inner(host, socket, session, TmuxSpawnMode::Attach).await
    }

    /// Start a control transport and create a new session inside it.
    pub async fn spawn_new_session(
        host: &TmuxHostConfig,
        socket: &TmuxSocket,
        session: &str,
        cols: u16,
        rows: u16,
    ) -> Result<TmuxControl> {
        Self::spawn_inner(
            host,
            socket,
            session,
            TmuxSpawnMode::NewSession { cols, rows },
        )
        .await
    }

    async fn spawn_inner(
        host: &TmuxHostConfig,
        socket: &TmuxSocket,
        session: &str,
        mode: TmuxSpawnMode,
    ) -> Result<TmuxControl> {
        if session.is_empty() {
            return Err(Error::Invalid("tmux session name must not be empty".into()));
        }

        let shared = Arc::new(Shared::new());
        let (transport, reader): (Arc<dyn TmuxTransport>, ReaderTask) = match host {
            TmuxHostConfig::Ssh { .. } => {
                spawn_ssh_transport(host, socket, session, mode, Arc::clone(&shared)).await?
            }
            TmuxHostConfig::Local | TmuxHostConfig::Wsl { .. } => {
                let prefix = process_prefix(host)
                    .ok_or_else(|| Error::Transport("missing process prefix".into()))?;
                spawn_process_transport(prefix, socket, session, mode, Arc::clone(&shared))?
            }
        };

        tracing::debug!(session, host = ?host, socket = ?socket, "tmux control: spawned");

        Ok(TmuxControl {
            session: session.to_string(),
            transport,
            reader: Mutex::new(Some(reader)),
            shared,
            command_lock: tokio::sync::Mutex::new(()),
        })
    }

    /// 发送一条 tmux 命令并等待对应的 `%begin/%end` 回复。
    ///
    /// 参数/输出：`self` 为输入；`command` 为输入命令；返回回复中的文本行。
    pub async fn send(&self, command: &str) -> Result<Vec<String>> {
        // Control-mode replies are correlated by `%begin/%end` encounter order.
        // Interleaving concurrent commands allows a resize/capture reply to be
        // claimed by the wrong waiter, so command round-trips must be serialized.
        let _command_guard = self.command_lock.lock().await;
        if self.shared.closed.load(Ordering::SeqCst) {
            return Err(gone(&self.session));
        }
        tracing::debug!(command, "tmux control: send");
        if self.shared.closed.load(Ordering::SeqCst) {
            return Err(gone(&self.session));
        }
        let (tx, rx) = oneshot::channel();
        lock(&self.shared.waiters).push_back(tx);

        if self.transport.write_command(command).await.is_err() {
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

    /// 订阅后续 tmux 结构事件。
    ///
    /// 参数/输出：`self` 为输入；返回事件接收端，订阅前的历史事件不会回放。
    pub fn subscribe(&self) -> UnboundedReceiver<TmuxEvent> {
        let (tx, rx) = mpsc::unbounded_channel();
        if !self.shared.closed.load(Ordering::SeqCst) {
            let mut events = lock(&self.shared.events);
            events.retain(|tx| !tx.is_closed());
            events.push(tx);
        }
        rx
    }

    /// 订阅指定 tmux pane 的后续原始输出。
    ///
    /// 参数/输出：`self` 和 `pane` 为输入；返回字节流接收端，订阅前的输出不会回放。
    pub fn output(&self, pane: &str) -> UnboundedReceiver<Vec<u8>> {
        let (tx, rx) = mpsc::unbounded_channel();
        if !self.shared.closed.load(Ordering::SeqCst) {
            let mut panes = lock(&self.shared.panes);
            panes.entry(pane.to_string()).or_default().push(tx);
        }
        rx
    }

    /// 判断控制连接是否已关闭。
    ///
    /// 参数/输出：`self` 为输入；返回关闭状态。
    pub fn is_closed(&self) -> bool {
        self.shared.closed.load(Ordering::SeqCst)
    }

    /// 停止传输后端和读取任务，并清理等待者与订阅者。
    ///
    /// 参数/输出：`self` 为输入输出，会清理全部连接资源；成功时无返回值。
    pub async fn close(&self) -> Result<()> {
        self.shared.closed.store(true, Ordering::SeqCst);
        let close_result = self.transport.close().await;
        let reader = lock(&self.reader).take();
        if let Some(reader) = reader {
            let _ = reader.await;
        }
        self.shared.shutdown();
        close_result
    }
}

impl Drop for TmuxControl {
    fn drop(&mut self) {
        if let Some(reader) = lock(&self.reader).take() {
            reader.abort();
        }
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

fn spawn_process_transport(
    prefix: Vec<String>,
    socket: &TmuxSocket,
    session: &str,
    mode: TmuxSpawnMode,
    shared: Arc<Shared>,
) -> Result<(Arc<dyn TmuxTransport>, ReaderTask)> {
    let argv = argv_for_mode(&prefix, socket, session, mode);
    tracing::debug!(argv = %argv.join(" "), session, "tmux control: spawn process");
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

    let (write_tx, write_rx) = mpsc::unbounded_channel::<String>();
    tokio::task::spawn_blocking(move || writer_loop(stdin, write_rx));
    let reader = tokio::task::spawn_blocking(move || reader_loop(stdout, shared));

    let transport = LocalTransport {
        child: Mutex::new(Some(child)),
        master: Mutex::new(Some(pair.master)),
        writer: Mutex::new(Some(write_tx)),
    };
    Ok((Arc::new(transport), reader))
}

async fn spawn_ssh_transport(
    host: &TmuxHostConfig,
    socket: &TmuxSocket,
    session: &str,
    mode: TmuxSpawnMode,
    shared: Arc<Shared>,
) -> Result<(Arc<dyn TmuxTransport>, ReaderTask)> {
    let TmuxHostConfig::Ssh {
        user,
        host,
        port,
        identity,
        password,
        options,
    } = host
    else {
        return Err(Error::Invalid(
            "ssh tmux transport requires ssh host".into(),
        ));
    };
    let command = remote_tmux_command(socket, session, mode);
    tracing::debug!(
        user,
        host,
        port,
        command,
        session,
        "tmux control: spawn ssh channel"
    );

    let client = connect_authenticated(
        user,
        host,
        *port,
        identity.as_deref(),
        password.as_deref(),
        options,
    )
    .await?;
    let channel = client
        .handle
        .channel_open_session()
        .await
        .map_err(|e| Error::Transport(format!("ssh channel_open_session: {e}")))?;
    channel
        .exec(true, command.as_bytes())
        .await
        .map_err(|e| Error::Transport(format!("ssh exec tmux control: {e}")))?;

    let (stdout, stdin) = tokio::io::split(channel.into_stream());
    let (write_tx, mut write_rx) = mpsc::unbounded_channel::<String>();
    tokio::spawn(async move {
        let mut stdin = stdin;
        while let Some(line) = write_rx.recv().await {
            if stdin.write_all(line.as_bytes()).await.is_err() {
                break;
            }
            if stdin.flush().await.is_err() {
                break;
            }
        }
        let _ = stdin.shutdown().await;
    });
    let reader = tokio::spawn(async move {
        async_reader_loop(stdout, shared).await;
    });

    let transport = SshTransport {
        handle: Mutex::new(Some(client)),
        writer: Mutex::new(Some(write_tx)),
    };
    Ok((Arc::new(transport), reader))
}

fn remote_tmux_command(socket: &TmuxSocket, session: &str, mode: TmuxSpawnMode) -> String {
    argv_for_mode(&[], socket, session, mode)
        .iter()
        .map(|arg| quote_arg(arg))
        .collect::<Vec<_>>()
        .join(" ")
}

struct ReaderState {
    open: Option<u64>,
    blocks: HashMap<u64, Vec<String>>,
    claimed: HashMap<u64, PendingReply>,
    handshake_done: bool,
}

impl ReaderState {
    fn new() -> Self {
        Self {
            open: None,
            blocks: HashMap::new(),
            claimed: HashMap::new(),
            handshake_done: false,
        }
    }
}

fn handle_parsed(state: &mut ReaderState, shared: &Shared, parsed: Parsed) -> bool {
    match parsed {
        Parsed::Begin { num, .. } => {
            tracing::trace!(num, "tmux control: begin");
            state.blocks.insert(num, Vec::new());
            state.open = Some(num);
            if state.handshake_done {
                if let Some(waiter) = shared.take_waiter() {
                    state.claimed.insert(num, waiter);
                }
            }
        }
        Parsed::End { num } => {
            tracing::trace!(num, "tmux control: end");
            let output = state.blocks.remove(&num).unwrap_or_default();
            if state.open == Some(num) {
                state.open = None;
            }
            if let Some(waiter) = state.claimed.remove(&num) {
                let _ = waiter.send(Ok(output));
            }
            state.handshake_done = true;
        }
        Parsed::Error { num, message } => {
            tracing::warn!(num, message, "tmux control: error");
            state.blocks.remove(&num);
            if state.open == Some(num) {
                state.open = None;
            }
            let message = if message.trim().is_empty() {
                format!("tmux command {num} failed")
            } else {
                message
            };
            if let Some(waiter) = state.claimed.remove(&num) {
                let _ = waiter.send(Err(Error::Transport(message)));
            }
            state.handshake_done = true;
        }
        Parsed::Output { pane, bytes } => {
            tracing::trace!(pane, len = bytes.len(), "tmux control: output");
            shared.deliver_output(&pane, bytes);
        }
        Parsed::ExtendedOutput { pane, bytes, .. } => {
            tracing::trace!(pane, len = bytes.len(), "tmux control: extended output");
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
                return false;
            }
        }
        Parsed::Unknown(text) => {
            if let Some(num) = state.open {
                if state.claimed.contains_key(&num) {
                    if let Some(output) = state.blocks.get_mut(&num) {
                        output.push(text);
                        return true;
                    }
                }
            }
        }
    }
    true
}

fn reader_loop<R: Read>(stdout: R, shared: Arc<Shared>) {
    let mut parser = ProtocolParser::new();
    let mut state = ReaderState::new();
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
            if !handle_parsed(&mut state, &shared, parsed) {
                return;
            }
        }
    }

    shared.shutdown();
}

async fn async_reader_loop<R: tokio::io::AsyncRead + Unpin>(stdout: R, shared: Arc<Shared>) {
    let mut parser = ProtocolParser::new();
    let mut state = ReaderState::new();
    let mut reader = TokioBufReader::new(stdout);
    let mut line: Vec<u8> = Vec::new();

    loop {
        line.clear();
        match reader.read_until(b'\n', &mut line).await {
            Ok(0) => {
                tracing::debug!("tmux control: channel EOF");
                break;
            }
            Ok(_) => {}
            Err(e) => {
                tracing::warn!(error = %e, "tmux control: channel read error");
                break;
            }
        }

        for parsed in parser.feed(&line) {
            if !handle_parsed(&mut state, &shared, parsed) {
                return;
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

    #[test]
    fn remote_command_quotes_socket_and_session() {
        assert_eq!(
            remote_tmux_command(
                &TmuxSocket::Named("x y".into()),
                "work space",
                TmuxSpawnMode::Attach
            ),
            "'tmux' '-L' 'x y' '-C' 'attach' '-t' 'work space'"
        );
    }
}
