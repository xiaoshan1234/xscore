//! Pane 输出 logger：独立订阅 pane 原始字节并写入本地文件。

use std::fs::{self, OpenOptions};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{anyhow, Context, Result};
use chrono::Local;
use tokio::io::{AsyncWrite, AsyncWriteExt};
use tokio::sync::broadcast;
use tokio::task::JoinHandle;
use xscore::domain::{PaneHandle, PaneId};

const MAX_CHAR_LINE_BYTES: usize = 64 * 1024;

/// logger 输出格式。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoggerFormat {
    /// 原样记录二进制输出。
    Raw,
    /// 过滤 ANSI/控制序列，按带时间戳的文本行记录。
    Char,
}

impl LoggerFormat {
    /// 返回用于默认文件名和命令输出的格式名称。
    ///
    /// 参数/输出：`self` 为输入；返回 `raw` 或 `char`。
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Raw => "raw",
            Self::Char => "char",
        }
    }
}

impl std::fmt::Display for LoggerFormat {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// 一个已启动 logger 的元数据。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoggerInfo {
    /// 被记录的 pane ID。
    pub pane_id: PaneId,
    /// 输出格式。
    pub format: LoggerFormat,
    /// raw 最大字节数或 char 最大行数。
    pub max_size: usize,
    /// 实际写入的本地文件绝对路径。
    pub path: PathBuf,
}

struct LoggerEntry {
    info: LoggerInfo,
    task: JoinHandle<()>,
}

/// daemon 持有的 logger 注册表。
#[derive(Default)]
pub struct LoggerRegistry {
    entries: Vec<LoggerEntry>,
}

impl LoggerRegistry {
    /// 创建空 logger 注册表。
    ///
    /// 参数/输出：无输入；返回空注册表。
    pub fn new() -> Self {
        Self::default()
    }

    /// 为 pane 启动一个 logger。
    ///
    /// 参数/输出：`handle`、`format`、`max_size` 和 `path` 为输入；返回 logger
    /// 元数据。raw 的 `max_size` 按字节计，char 按输出行数计。
    pub fn start(
        &mut self,
        handle: Arc<dyn PaneHandle>,
        format: LoggerFormat,
        max_size: usize,
        path: PathBuf,
    ) -> Result<LoggerInfo> {
        if max_size == 0 {
            return Err(anyhow!("logger max-size must be greater than zero"));
        }
        create_parent(&path)?;
        let file = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&path)
            .with_context(|| format!("create logger file {}", path.display()))?;
        let file_path = fs::canonicalize(&path).unwrap_or(path);
        let pane_id = handle.id();
        let mut receiver = handle.subscribe();
        let task_path = file_path.clone();
        let task = tokio::spawn(async move {
            let mut file = tokio::fs::File::from_std(file);
            let result = match format {
                LoggerFormat::Raw => write_raw(&mut file, &mut receiver, max_size).await,
                LoggerFormat::Char => write_char(&mut file, &mut receiver, max_size).await,
            };
            if let Err(error) = result {
                tracing::warn!(
                    pane = %pane_id,
                    path = %task_path.display(),
                    %error,
                    "pane logger stopped with error"
                );
            }
        });
        let info = LoggerInfo {
            pane_id,
            format,
            max_size,
            path: file_path,
        };
        self.entries.push(LoggerEntry {
            info: info.clone(),
            task,
        });
        Ok(info)
    }

    /// 返回当前登记的 logger 数量。
    ///
    /// 参数/输出：`self` 为输入；返回 logger 数量。
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// 判断当前是否没有 logger。
    ///
    /// 参数/输出：`self` 为输入；返回 logger 数量是否为零。
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// 停止并移除指定 pane 的全部 logger。
    ///
    /// 参数/输出：`self` 为输入输出；`pane_id` 为输入；不返回值。
    pub fn remove_pane(&mut self, pane_id: PaneId) {
        for entry in &self.entries {
            if entry.info.pane_id == pane_id {
                entry.task.abort();
            }
        }
        self.entries.retain(|entry| entry.info.pane_id != pane_id);
    }
}

/// 生成默认 logger 文件路径。
///
/// 参数/输出：`pane_id` 和 `format` 为输入；返回当前目录下的绝对文件路径。
pub fn default_path(pane_id: PaneId, format: LoggerFormat) -> Result<PathBuf> {
    let timestamp = Local::now().format("%Y%m%dT%H%M%S%.3f");
    Ok(std::env::current_dir()
        .context("resolve current directory")?
        .join(format!(
            "xscore-pane-{pane_id}-{timestamp}.{}.log",
            format.as_str()
        )))
}

fn create_parent(path: &Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent)
                .with_context(|| format!("create logger directory {}", parent.display()))?;
        }
    }
    Ok(())
}

async fn write_raw<W>(
    file: &mut W,
    receiver: &mut broadcast::Receiver<Vec<u8>>,
    max_bytes: usize,
) -> Result<()>
where
    W: AsyncWrite + Unpin,
{
    let mut written = 0usize;
    while written < max_bytes {
        match receiver.recv().await {
            Ok(chunk) => {
                let remaining = max_bytes - written;
                let length = chunk.len().min(remaining);
                file.write_all(&chunk[..length]).await?;
                file.flush().await?;
                written += length;
            }
            Err(broadcast::error::RecvError::Lagged(skipped)) => {
                return Err(anyhow!("raw logger lagged and skipped {skipped} chunks"));
            }
            Err(broadcast::error::RecvError::Closed) => break,
        }
    }
    file.flush().await?;
    Ok(())
}

async fn write_char<W>(
    file: &mut W,
    receiver: &mut broadcast::Receiver<Vec<u8>>,
    max_lines: usize,
) -> Result<()>
where
    W: AsyncWrite + Unpin,
{
    let mut filter = CharFilter::default();
    let mut written = 0usize;

    'recording: loop {
        match receiver.recv().await {
            Ok(chunk) => {
                for line in filter.push(&chunk) {
                    file.write_all(line.as_bytes()).await?;
                    file.write_all(b"\n").await?;
                    file.flush().await?;
                    written += 1;
                    if written >= max_lines {
                        break 'recording;
                    }
                }
            }
            Err(broadcast::error::RecvError::Lagged(skipped)) => {
                return Err(anyhow!("char logger lagged and skipped {skipped} chunks"));
            }
            Err(broadcast::error::RecvError::Closed) => {
                if let Some(line) = filter.finish() {
                    if written < max_lines {
                        file.write_all(line.as_bytes()).await?;
                        file.write_all(b"\n").await?;
                    }
                }
                break;
            }
        }
    }
    file.flush().await?;
    Ok(())
}

#[derive(Debug, Clone, Copy, Default)]
enum FilterState {
    #[default]
    Ground,
    Escape,
    Csi,
    Osc,
    Str,
    StrEscape,
}

#[derive(Default)]
struct CharFilter {
    state: FilterState,
    line: Vec<u8>,
    timestamp: Option<String>,
}

impl CharFilter {
    fn push(&mut self, bytes: &[u8]) -> Vec<String> {
        let mut lines = Vec::new();
        for &byte in bytes {
            match self.state {
                FilterState::Ground => self.push_ground(byte, &mut lines),
                FilterState::Escape => match byte {
                    b'[' => self.state = FilterState::Csi,
                    b']' => self.state = FilterState::Osc,
                    b'P' | b'X' | b'^' | b'_' => self.state = FilterState::Str,
                    0x20..=0x2f => {}
                    _ => self.state = FilterState::Ground,
                },
                FilterState::Csi => {
                    if (0x40..=0x7e).contains(&byte) {
                        self.state = FilterState::Ground;
                    }
                }
                FilterState::Osc => match byte {
                    0x07 => self.state = FilterState::Ground,
                    0x1b => self.state = FilterState::StrEscape,
                    _ => {}
                },
                FilterState::Str => {
                    if byte == 0x1b {
                        self.state = FilterState::StrEscape;
                    }
                }
                FilterState::StrEscape => {
                    self.state = if byte == b'\\' {
                        FilterState::Ground
                    } else {
                        FilterState::Str
                    };
                }
            }
            if self.line.len() >= MAX_CHAR_LINE_BYTES {
                if let Some(line) = self.finish() {
                    lines.push(line);
                }
            }
        }
        lines
    }

    fn push_ground(&mut self, byte: u8, lines: &mut Vec<String>) {
        match byte {
            0x1b => self.state = FilterState::Escape,
            b'\n' | b'\r' => {
                if let Some(line) = self.finish() {
                    lines.push(line);
                }
            }
            b'\t' => self.append(b"    "),
            0x08 => self.pop_char(),
            0x00..=0x1f | 0x7f => {}
            _ => self.append(&[byte]),
        }
    }

    fn append(&mut self, bytes: &[u8]) {
        if self.line.is_empty() {
            self.timestamp = Some(Local::now().format("%Y-%m-%d %H:%M:%S%.3f").to_string());
        }
        self.line.extend_from_slice(bytes);
    }

    fn pop_char(&mut self) {
        let Some(start) = self.line.iter().rposition(|byte| byte & 0xc0 != 0x80) else {
            return;
        };
        self.line.truncate(start);
    }

    fn finish(&mut self) -> Option<String> {
        if self.line.is_empty() {
            self.timestamp = None;
            return None;
        }
        let timestamp = self
            .timestamp
            .take()
            .unwrap_or_else(|| Local::now().format("%Y-%m-%d %H:%M:%S%.3f").to_string());
        let raw_line = std::mem::take(&mut self.line);
        let text = String::from_utf8_lossy(&raw_line);
        Some(format!("[{timestamp}] {text}"))
    }
}

#[cfg(test)]
mod tests {
    use std::any::Any;
    use std::path::{Path, PathBuf};
    use std::pin::Pin;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::Arc;
    use std::task::{Context, Poll};
    use std::time::Duration;

    use async_trait::async_trait;
    use tokio::io::AsyncWrite;
    use tokio::sync::broadcast;
    use tokio::time::{sleep, timeout, Instant};
    use xscore::domain::{PaneHandle, PaneId, PaneKind};
    use xscore::platform::Size;

    use super::{write_char, write_raw, CharFilter, LoggerFormat, LoggerRegistry};

    static NEXT_FILE: AtomicU64 = AtomicU64::new(0);

    struct FakePane {
        id: PaneId,
        output: broadcast::Sender<Vec<u8>>,
    }

    #[async_trait]
    impl PaneHandle for FakePane {
        fn id(&self) -> PaneId {
            self.id
        }

        fn kind(&self) -> PaneKind {
            PaneKind::Local
        }

        fn as_any(&self) -> &dyn Any {
            self
        }

        async fn send_input(&self, data: &[u8]) -> xscore::Result<usize> {
            Ok(data.len())
        }

        async fn resize(&self, _size: Size) -> xscore::Result<()> {
            Ok(())
        }

        fn subscribe(&self) -> broadcast::Receiver<Vec<u8>> {
            self.output.subscribe()
        }

        async fn close(&self) -> xscore::Result<()> {
            Ok(())
        }
    }

    #[derive(Default)]
    struct BufferWriter(Vec<u8>);

    impl AsyncWrite for BufferWriter {
        fn poll_write(
            mut self: Pin<&mut Self>,
            _context: &mut Context<'_>,
            bytes: &[u8],
        ) -> Poll<std::io::Result<usize>> {
            self.0.extend_from_slice(bytes);
            Poll::Ready(Ok(bytes.len()))
        }

        fn poll_flush(
            self: Pin<&mut Self>,
            _context: &mut Context<'_>,
        ) -> Poll<std::io::Result<()>> {
            Poll::Ready(Ok(()))
        }

        fn poll_shutdown(
            self: Pin<&mut Self>,
            _context: &mut Context<'_>,
        ) -> Poll<std::io::Result<()>> {
            Poll::Ready(Ok(()))
        }
    }

    #[test]
    fn char_filter_strips_ansi_and_control_sequences() {
        let mut filter = CharFilter::default();
        let lines = filter.push(b"\x1b[31mhe\x1b[0mllo\r\nworld\x07\n");
        assert_eq!(lines.len(), 2);
        assert!(lines[0].ends_with("] hello"));
        assert!(lines[1].ends_with("] world"));
    }

    #[test]
    fn char_filter_keeps_split_utf8_until_line_end() {
        let mut filter = CharFilter::default();
        assert!(filter.push(&[0xe4, 0xb8]).is_empty());
        let lines = filter.push(b"\xad\n");
        assert_eq!(lines.len(), 1);
        assert!(lines[0].ends_with("] 中"));
    }

    #[tokio::test]
    async fn raw_writer_stops_at_byte_limit() {
        let (sender, mut receiver) = tokio::sync::broadcast::channel(4);
        sender.send(vec![b'a'; 1500]).unwrap();
        sender.send(vec![b'b'; 1000]).unwrap();
        drop(sender);

        let mut writer = BufferWriter::default();
        write_raw(&mut writer, &mut receiver, 2000).await.unwrap();

        assert_eq!(writer.0.len(), 2000);
        assert_eq!(writer.0.iter().filter(|byte| **byte == b'a').count(), 1500);
        assert_eq!(writer.0.iter().filter(|byte| **byte == b'b').count(), 500);
    }

    #[tokio::test]
    async fn char_writer_stops_at_line_limit() {
        let (sender, mut receiver) = tokio::sync::broadcast::channel(4096);
        for _ in 0..2100 {
            sender.send(b"line\n".to_vec()).unwrap();
        }
        drop(sender);

        let mut writer = BufferWriter::default();
        write_char(&mut writer, &mut receiver, 2000).await.unwrap();

        assert_eq!(writer.0.iter().filter(|byte| **byte == b'\n').count(), 2000);
    }

    #[tokio::test]
    async fn registry_raw_logger_ignores_output_after_byte_limit() {
        let (pane, sender, path) = fake_pane("raw-overflow");
        let mut loggers = LoggerRegistry::new();
        loggers
            .start(pane, LoggerFormat::Raw, 2000, path.clone())
            .unwrap();

        sender.send(vec![b'a'; 1500]).unwrap();
        sender.send(vec![b'b'; 1000]).unwrap();
        wait_for_bytes(&path, 2000).await;

        let _ = sender.send(vec![b'c'; 500]);
        sleep(Duration::from_millis(50)).await;
        let bytes = std::fs::read(&path).unwrap();
        assert_eq!(bytes.len(), 2000);
        assert_eq!(bytes.iter().filter(|byte| **byte == b'a').count(), 1500);
        assert_eq!(bytes.iter().filter(|byte| **byte == b'b').count(), 500);
        assert!(!bytes.contains(&b'c'));

        let _ = std::fs::remove_file(path);
    }

    #[tokio::test]
    async fn registry_char_logger_ignores_output_after_line_limit() {
        let (pane, sender, path) = fake_pane("char-overflow");
        let mut loggers = LoggerRegistry::new();
        loggers
            .start(pane, LoggerFormat::Char, 3, path.clone())
            .unwrap();

        sender.send(b"one\ntwo\nthree\nfour\n".to_vec()).unwrap();
        wait_for_lines(&path, 3).await;

        let _ = sender.send(b"five\nsix\n".to_vec());
        sleep(Duration::from_millis(50)).await;
        let text = std::fs::read_to_string(&path).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 3);
        assert!(lines[0].ends_with("] one"));
        assert!(lines[1].ends_with("] two"));
        assert!(lines[2].ends_with("] three"));
        assert!(!text.contains("four"));
        assert!(!text.contains("five"));

        let _ = std::fs::remove_file(path);
    }

    fn fake_pane(label: &str) -> (Arc<dyn PaneHandle>, broadcast::Sender<Vec<u8>>, PathBuf) {
        let (sender, _) = broadcast::channel(16);
        let pane = Arc::new(FakePane {
            id: PaneId::new(NEXT_FILE.fetch_add(1, Ordering::Relaxed) + 1),
            output: sender.clone(),
        });
        let path = std::env::temp_dir().join(format!(
            "xscore-{label}-{}-{}.log",
            std::process::id(),
            NEXT_FILE.fetch_add(1, Ordering::Relaxed)
        ));
        (pane, sender, path)
    }

    async fn wait_for_bytes(path: &Path, expected: u64) {
        timeout(Duration::from_secs(2), async {
            loop {
                if std::fs::metadata(path)
                    .map(|metadata| metadata.len() == expected)
                    .unwrap_or(false)
                {
                    return;
                }
                sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("logger did not reach byte limit");
    }

    async fn wait_for_lines(path: &Path, expected: usize) {
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            if std::fs::read_to_string(path)
                .map(|text| text.lines().count() == expected)
                .unwrap_or(false)
            {
                return;
            }
            assert!(Instant::now() < deadline, "logger did not reach line limit");
            sleep(Duration::from_millis(10)).await;
        }
    }
}
