//! L0 平台抽象：本地 PTY/ConPTY。
//!
//! 该层不理解 tmux、SSH 或任何上层协议；它只负责“在 PTY 上启动一个程序”并
//! 提供读/写/改尺寸。`tmux -CC attach` 在本层就是一个普通子进程。

use crate::{Error, Result};
use portable_pty::{native_pty_system, Child, CommandBuilder, MasterPty, PtySize};
use std::io::{Read, Write};
use std::sync::Mutex as StdMutex;
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender};

pub mod ssh;
pub mod tmux;

/// 窗格/终端尺寸（字符行列）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Size {
    pub cols: u16,
    pub rows: u16,
}

impl Size {
    pub fn new(cols: u16, rows: u16) -> Self {
        Self { cols, rows }
    }
}

const READ_CHUNK: usize = 8 * 1024;

/// 本地 PTY 传输。读/写各用一条 OS 线程，异步侧通过无界通道对接，读写互不
/// 阻塞。
pub struct LocalPty {
    command: String,
    args: Vec<String>,
    cwd: Option<String>,
    master: StdMutex<Option<Box<dyn MasterPty + Send>>>,
    child: StdMutex<Option<Box<dyn Child + Send + Sync>>>,
    to_child: StdMutex<Option<UnboundedSender<Vec<u8>>>>,
}

impl std::fmt::Debug for LocalPty {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LocalPty")
            .field("command", &self.command)
            .field("args", &self.args)
            .field("cwd", &self.cwd)
            .finish_non_exhaustive()
    }
}

impl LocalPty {
    pub fn new(command: impl Into<String>, args: Vec<String>, cwd: Option<String>) -> Self {
        Self {
            command: command.into(),
            args,
            cwd,
            master: StdMutex::new(None),
            child: StdMutex::new(None),
            to_child: StdMutex::new(None),
        }
    }

    /// 打开 PTY、启动子进程，返回子进程输出字节流（每段一个 chunk）。
    pub fn spawn(&self, size: Size) -> Result<UnboundedReceiver<Vec<u8>>> {
        tracing::debug!(
            command = %self.command,
            args = ?self.args,
            cols = size.cols,
            rows = size.rows,
            "pty spawn"
        );
        if self.master.lock().unwrap().is_some() {
            return Err(Error::Invalid("pty already spawned".into()));
        }

        let pair = native_pty_system()
            .openpty(PtySize {
                rows: size.rows,
                cols: size.cols,
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
        if cmd.get_env("TERM").is_none() {
            cmd.env("TERM", "xterm-256color");
        }

        let child = pair
            .slave
            .spawn_command(cmd)
            .map_err(|e| Error::Transport(format!("spawn: {e}")))?;
        // 释放 slave，让 pty 在子进程退出时报告 EOF。
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
            .name("emulator-pty-read".into())
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
                        Err(_) => break,
                    }
                }
            })
            .map_err(|e| Error::Transport(format!("pty reader thread: {e}")))?;

        let (in_tx, mut in_rx) = unbounded_channel::<Vec<u8>>();
        std::thread::Builder::new()
            .name("emulator-pty-write".into())
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

        Ok(out_rx)
    }

    pub fn write(&self, data: &[u8]) -> Result<usize> {
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

    pub fn resize(&self, size: Size) -> Result<()> {
        let guard = self.master.lock().unwrap();
        let master = guard
            .as_ref()
            .ok_or_else(|| Error::Transport("not open".into()))?;
        master
            .resize(PtySize {
                rows: size.rows,
                cols: size.cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| Error::Transport(format!("pty resize: {e}")))
    }

    pub fn close(&self) -> Result<()> {
        // 先关 writer：结束写线程并释放重复的 master fd。
        *self.to_child.lock().unwrap() = None;
        let child = self.child.lock().unwrap().take();
        if let Some(mut child) = child {
            let _ = child.kill();
            let _ = child.wait();
        }
        // 丢弃 master，让 reader 线程的阻塞 read 返回。
        *self.master.lock().unwrap() = None;
        Ok(())
    }
}

impl Drop for LocalPty {
    fn drop(&mut self) {
        let _ = self.close();
    }
}
