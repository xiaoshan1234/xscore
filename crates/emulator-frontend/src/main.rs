//! CLI 模式前端（L3–L5 模拟）：一个进程内维护 Domain 与 pane，提供操作与数据
//! 读取。

use anyhow::{anyhow, Context, Result};
use emulator::domain::{Domain, LocalDomain, PaneConfig, PaneHandle, PaneId, PaneKind};
use emulator::platform::Size;
use emulator::term::{TerminalCore, VtTerm};
use std::collections::HashMap;
use std::io::Write as _;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::Mutex;

struct PaneEntry {
    handle: Arc<dyn PaneHandle>,
    term: Arc<Mutex<VtTerm>>,
}

struct App {
    panes: HashMap<PaneId, PaneEntry>,
}

impl App {
    fn new() -> Self {
        Self {
            panes: HashMap::new(),
        }
    }

    async fn shell(&mut self, words: &[String]) -> Result<()> {
        if words.len() < 2 {
            return Err(anyhow!("usage: shell <program> [args...]"));
        }
        let cols = 80u16;
        let rows = 24u16;

        // LocalDomain 是 1:1 域：一个连接对应一个 pane。
        let domain = LocalDomain::new();
        let handle = domain
            .spawn_pane(&PaneConfig {
                command: words[1].clone(),
                args: words[2..].to_vec(),
                cwd: None,
                cols,
                rows,
            })
            .await?;
        let id = handle.id();

        let term = Arc::new(Mutex::new(VtTerm::new(cols, rows)));
        let mut rx = handle.subscribe();
        let pump = term.clone();
        tokio::spawn(async move {
            while let Ok(chunk) = rx.recv().await {
                pump.lock().await.feed(&chunk);
            }
        });

        self.panes.insert(id, PaneEntry { handle, term });
        println!("pane: {id}  kind: local  {cols}x{rows}");
        Ok(())
    }

    async fn write(&self, words: &[String]) -> Result<()> {
        if words.len() < 3 {
            return Err(anyhow!("usage: write <pane-id> <text>"));
        }
        let entry = self.get_pane(&words[1])?;
        let mut data = words[2..].join(" ").into_bytes();
        data.push(b'\n');
        entry.handle.send_input(&data).await?;
        Ok(())
    }

    async fn read(&self, words: &[String]) -> Result<()> {
        if words.len() < 2 {
            return Err(anyhow!("usage: read <pane-id>"));
        }
        let entry = self.get_pane(&words[1])?;
        // 给后台 pump 一个极短的静默窗口，让刚到达的字节先喂进网格。
        tokio::time::sleep(Duration::from_millis(60)).await;
        let snap = entry.term.lock().await.snapshot();
        let mut out = std::io::stdout();
        out.write_all(snap.as_bytes())?;
        if !snap.ends_with('\n') {
            out.write_all(b"\n")?;
        }
        out.flush()?;
        Ok(())
    }

    async fn resize(&self, words: &[String]) -> Result<()> {
        if words.len() < 4 {
            return Err(anyhow!("usage: resize <pane-id> <cols> <rows>"));
        }
        let entry = self.get_pane(&words[1])?;
        let cols: u16 = words[2].parse().context("cols")?;
        let rows: u16 = words[3].parse().context("rows")?;
        entry.handle.resize(Size::new(cols, rows)).await?;
        entry.term.lock().await.resize(cols, rows);
        Ok(())
    }

    async fn list(&self) {
        println!("{:<10} {:<8} {:<6} {:<6}", "PANE", "KIND", "COLS", "ROWS");
        let mut ids: Vec<PaneId> = self.panes.keys().copied().collect();
        ids.sort_by_key(|p| p.as_u64());
        for id in ids {
            let entry = &self.panes[&id];
            let (cols, rows) = {
                let term = entry.term.lock().await;
                (term.cols(), term.rows())
            };
            let kind = match entry.handle.kind() {
                PaneKind::Local => "local",
                PaneKind::Ssh => "ssh",
                PaneKind::Tmux => "tmux",
            };
            println!("{:<10} {:<8} {:<6} {:<6}", id, kind, cols, rows);
        }
    }

    fn get_pane(&self, s: &str) -> Result<&PaneEntry> {
        let id = PaneId::new(s.parse().context("pane id")?);
        self.panes
            .get(&id)
            .ok_or_else(|| anyhow!("pane not found: {id}"))
    }
}

fn help() {
    println!("commands:");
    println!("  shell <program> [args...]   spawn a local shell pane");
    println!("  write <pane-id> <text>      send text + newline to the pane");
    println!("  read  <pane-id>             print accumulated output");
    println!("  resize <pane-id> <c> <r>    resize the pane");
    println!("  list                        list panes");
    println!("  quit | exit                 leave");
}

async fn dispatch(app: &mut App, words: &[String]) -> Result<bool> {
    match words[0].as_str() {
        "shell" => app.shell(words).await?,
        "write" => app.write(words).await?,
        "read" => app.read(words).await?,
        "resize" => app.resize(words).await?,
        "list" => app.list().await,
        "help" => help(),
        "quit" | "exit" => return Ok(true),
        other => println!("unknown command: {other} (try `help`)"),
    }
    Ok(false)
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let _ = tracing_subscriber::fmt::try_init();
    let mut app = App::new();
    let stdin = tokio::io::stdin();
    let mut lines = BufReader::new(stdin).lines();

    loop {
        print!("xscore> ");
        std::io::stdout().flush()?;
        let Some(line) = lines.next_line().await? else {
            break;
        };
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let words: Vec<String> = line.split_whitespace().map(str::to_string).collect();
        match dispatch(&mut app, &words).await {
            Ok(true) => break,
            Ok(false) => {}
            Err(e) => eprintln!("error: {e:#}"),
        }
    }

    Ok(())
}
