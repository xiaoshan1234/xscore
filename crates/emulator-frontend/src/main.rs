//! CLI 模式前端（L3–L5 模拟）：一个进程内维护 Domain 与 pane，提供操作与数据
//! 读取。

mod layout;

use anyhow::{anyhow, Context, Result};
use emulator::domain::{
    Domain, LocalDomain, PaneConfig, PaneHandle, PaneId, PaneKind, RemoteSshDomain, TmuxDomain,
};
use emulator::platform::tmux::{TmuxHostConfig, TmuxSocket};
use emulator::platform::Size;
use emulator::term::{TerminalCore, VtTerm};
use std::collections::HashMap;
use std::io::Write as _;
use std::sync::Arc;
use std::sync::Mutex as StdMutex;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::Mutex;
use tracing_subscriber::filter::LevelFilter;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::EnvFilter;
use tracing_subscriber::Layer;

use layout::{Direction, Layout, TmuxContainer};

fn init_logging() -> Result<()> {
    let path = std::env::var("XSCORE_LOG").unwrap_or_else(|_| "emulator.log".to_string());
    let file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .with_context(|| format!("open log file {path}"))?;

    let file_layer = tracing_subscriber::fmt::layer()
        .with_writer(StdMutex::new(file))
        .with_ansi(false);
    let stderr_layer = tracing_subscriber::fmt::layer()
        .with_writer(std::io::stderr)
        .with_filter(LevelFilter::WARN);
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("emulator=debug,emulator_frontend=debug"));

    tracing_subscriber::registry()
        .with(filter)
        .with(file_layer)
        .with(stderr_layer)
        .try_init()
        .context("init tracing subscriber")?;
    Ok(())
}

struct PaneEntry {
    handle: Arc<dyn PaneHandle>,
    term: Arc<Mutex<VtTerm>>,
}

struct App {
    panes: HashMap<PaneId, PaneEntry>,
    layout: Layout,
    tmux_window: HashMap<PaneId, String>,
}

impl App {
    fn new() -> Self {
        Self {
            panes: HashMap::new(),
            layout: Layout::new(),
            tmux_window: HashMap::new(),
        }
    }

    async fn shell(&mut self, words: &[String]) -> Result<()> {
        if words.len() < 2 {
            return Err(anyhow!("usage: shell <program> [args...]"));
        }
        let id = self
            .spawn_local(words[1].clone(), words[2..].to_vec())
            .await?;
        self.layout.add(id);
        println!("pane: {id}  kind: local  80x24");
        Ok(())
    }

    async fn split(&mut self, words: &[String]) -> Result<()> {
        if words.len() < 3 {
            return Err(anyhow!("usage: split h|v <program> [args...]"));
        }
        let direction = match words[1].as_str() {
            "h" => Direction::Horizontal,
            "v" => Direction::Vertical,
            other => return Err(anyhow!("split direction must be h or v, got {other}")),
        };
        let id = self
            .spawn_local(words[2].clone(), words[3..].to_vec())
            .await?;
        self.layout.split_focused(direction, id);
        println!("pane: {id}  kind: local  80x24");
        Ok(())
    }

    async fn spawn_local(&mut self, command: String, args: Vec<String>) -> Result<PaneId> {
        let cols = 80u16;
        let rows = 24u16;
        let domain = LocalDomain::new();
        let handle = domain
            .spawn_pane(&PaneConfig::Local {
                command,
                args,
                cwd: None,
                cols,
                rows,
            })
            .await?;
        Ok(self.register_pane(handle, cols, rows))
    }

    async fn tmux(&mut self, words: &[String]) -> Result<()> {
        let (session, socket, host) = parse_tmux_args(words)?;
        let domain = TmuxDomain::new();
        let panes = domain.attach(&host, &socket, &session).await?;

        let mut groups: Vec<(String, Vec<PaneId>)> = Vec::new();
        let mut index: HashMap<String, usize> = HashMap::new();
        for pane in panes {
            let (cols, rows) = (pane.cols(), pane.rows());
            let window = pane.window().to_string();
            let id = self.register_pane(pane, cols, rows);
            self.tmux_window.insert(id, window.clone());
            let group = *index.entry(window.clone()).or_insert_with(|| {
                groups.push((window, Vec::new()));
                groups.len() - 1
            });
            groups[group].1.push(id);
        }
        for (window, pane_ids) in groups {
            let active = pane_ids.first().copied();
            let container = TmuxContainer {
                window: window.clone(),
                panes: pane_ids.clone(),
                active,
            };
            self.layout.add_mirror(container);
            println!("tmux window {window}: {} pane(s)", pane_ids.len());
        }
        Ok(())
    }

    async fn ssh(&mut self, words: &[String]) -> Result<()> {
        if words.len() < 2 {
            return Err(anyhow!("usage: ssh <user@host[:port]> [identity]"));
        }
        let (user, host, port) = parse_ssh_target(&words[1])?;
        let identity = words.get(2).cloned();
        let cols = 80u16;
        let rows = 24u16;
        let domain = RemoteSshDomain::new();
        let handle = domain
            .spawn_pane(&PaneConfig::Ssh {
                host,
                port,
                user,
                identity,
                cols,
                rows,
            })
            .await?;
        let id = self.attach_pane(handle, cols, rows);
        println!("pane: {id}  kind: ssh  {cols}x{rows}");
        Ok(())
    }

    async fn wsl(&mut self, words: &[String]) -> Result<()> {
        let args = words[1..].to_vec();
        let id = self.spawn_local("wsl.exe".to_string(), args).await?;
        self.layout.add(id);
        println!("pane: {id}  kind: wsl  80x24");
        Ok(())
    }

    fn register_pane(&mut self, handle: Arc<dyn PaneHandle>, cols: u16, rows: u16) -> PaneId {
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
        id
    }

    fn attach_pane(&mut self, handle: Arc<dyn PaneHandle>, cols: u16, rows: u16) -> PaneId {
        let id = self.register_pane(handle, cols, rows);
        self.layout.add(id);
        id
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

    fn focus(&mut self, words: &[String]) -> Result<()> {
        if words.len() < 2 {
            return Err(anyhow!("usage: focus <pane-id>"));
        }
        let id = PaneId::new(words[1].parse().context("pane id")?);
        if !self.panes.contains_key(&id) {
            return Err(anyhow!("pane not found: {id}"));
        }
        if let Some(window) = self.tmux_window.get(&id).cloned() {
            if !self.layout.set_mirror_active(&window, id) {
                return Err(anyhow!("tmux window not found in layout: {window}"));
            }
        } else {
            self.layout.focus(id);
        }
        println!("focused: {id}");
        Ok(())
    }

    fn layout_cmd(&self) {
        println!("{}", self.layout.render());
        if let Some(focused) = self.layout.focused() {
            println!("focused: {focused}");
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
    println!("  ssh <user@host[:port]> [identity]   spawn a remote ssh pane");
    println!("  wsl [args...]               spawn a WSL shell pane (Windows)");
    println!("  tmux <session> [-L <socket>] [--ssh <dest> | --wsl]   attach to a tmux session");
    println!("  split h|v <program> [args...]   split the focused pane");
    println!("  write <pane-id> <text>      send text + newline to the pane");
    println!("  read  <pane-id>             print accumulated output");
    println!("  resize <pane-id> <c> <r>    resize the pane");
    println!("  focus <pane-id>             move focus");
    println!("  layout                      print the pane tree");
    println!("  list                        list panes");
    println!("  quit | exit                 leave");
}

fn parse_ssh_target(target: &str) -> Result<(String, String, u16)> {
    let (user, rest) = target
        .split_once('@')
        .ok_or_else(|| anyhow!("expected user@host[:port], got {target}"))?;
    let (host, port) = match rest.split_once(':') {
        Some((host, port)) => (host.to_string(), port.parse::<u16>().context("port")?),
        None => (rest.to_string(), 22),
    };
    if user.is_empty() || host.is_empty() {
        return Err(anyhow!("invalid ssh target {target}"));
    }
    Ok((user.to_string(), host, port))
}

fn parse_tmux_args(words: &[String]) -> Result<(String, TmuxSocket, TmuxHostConfig)> {
    if words.len() < 2 {
        return Err(anyhow!(
            "usage: tmux <session> [-L <name> | -S <path>] [--ssh <dest> | --wsl] [--*-arg <arg>]"
        ));
    }
    let session = words[1].clone();
    let mut socket = TmuxSocket::Default;
    let mut host = TmuxHostConfig::Local;
    let mut ssh_seen = false;
    let mut wsl_seen = false;
    let mut i = 2;
    while i < words.len() {
        match words[i].as_str() {
            "-L" => {
                i += 1;
                let name = words
                    .get(i)
                    .ok_or_else(|| anyhow!("-L requires a socket name"))?;
                socket = TmuxSocket::Named(name.clone());
            }
            "-S" => {
                i += 1;
                let path = words
                    .get(i)
                    .ok_or_else(|| anyhow!("-S requires a socket path"))?;
                socket = TmuxSocket::Path(path.clone());
            }
            "--ssh" => {
                if ssh_seen {
                    return Err(anyhow!("--ssh may only be given once"));
                }
                if wsl_seen {
                    return Err(anyhow!("--ssh and --wsl are mutually exclusive"));
                }
                i += 1;
                let destination = words
                    .get(i)
                    .ok_or_else(|| anyhow!("--ssh requires a destination"))?;
                host = TmuxHostConfig::Ssh {
                    destination: destination.clone(),
                    args: Vec::new(),
                };
                ssh_seen = true;
            }
            "--ssh-arg" => {
                i += 1;
                let arg = words
                    .get(i)
                    .ok_or_else(|| anyhow!("--ssh-arg requires an argument"))?;
                match &mut host {
                    TmuxHostConfig::Ssh { args, .. } => args.push(arg.clone()),
                    _ => return Err(anyhow!("--ssh-arg requires --ssh first")),
                }
            }
            "--wsl" => {
                if wsl_seen {
                    return Err(anyhow!("--wsl may only be given once"));
                }
                if ssh_seen {
                    return Err(anyhow!("--ssh and --wsl are mutually exclusive"));
                }
                host = TmuxHostConfig::Wsl { args: Vec::new() };
                wsl_seen = true;
            }
            "--wsl-arg" => {
                i += 1;
                let arg = words
                    .get(i)
                    .ok_or_else(|| anyhow!("--wsl-arg requires an argument"))?;
                match &mut host {
                    TmuxHostConfig::Wsl { args } => args.push(arg.clone()),
                    _ => return Err(anyhow!("--wsl-arg requires --wsl first")),
                }
            }
            other => return Err(anyhow!("unknown tmux option: {other}")),
        }
        i += 1;
    }
    Ok((session, socket, host))
}

async fn dispatch(app: &mut App, words: &[String]) -> Result<bool> {
    match words[0].as_str() {
        "shell" => app.shell(words).await?,
        "ssh" => app.ssh(words).await?,
        "wsl" => app.wsl(words).await?,
        "tmux" => app.tmux(words).await?,
        "split" => app.split(words).await?,
        "write" => app.write(words).await?,
        "read" => app.read(words).await?,
        "resize" => app.resize(words).await?,
        "focus" => app.focus(words)?,
        "layout" => app.layout_cmd(),
        "list" => app.list().await,
        "help" => help(),
        "quit" | "exit" => return Ok(true),
        other => println!("unknown command: {other} (try `help`)"),
    }
    Ok(false)
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    if let Err(e) = init_logging() {
        eprintln!("warning: logging disabled: {e:#}");
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ssh_target_with_default_port() {
        let (user, host, port) = parse_ssh_target("dev@box").unwrap();
        assert_eq!((user.as_str(), host.as_str(), port), ("dev", "box", 22));
    }

    #[test]
    fn ssh_target_with_port() {
        let (user, host, port) = parse_ssh_target("dev@box:2222").unwrap();
        assert_eq!((user.as_str(), host.as_str(), port), ("dev", "box", 2222));
    }

    #[test]
    fn ssh_target_requires_user() {
        assert!(parse_ssh_target("box").is_err());
    }

    fn words(input: &str) -> Vec<String> {
        input.split_whitespace().map(str::to_string).collect()
    }

    #[test]
    fn tmux_args_default_local() {
        let (session, socket, host) = parse_tmux_args(&words("tmux work -L x")).unwrap();
        assert_eq!(session, "work");
        assert_eq!(socket, TmuxSocket::Named("x".into()));
        assert_eq!(host, TmuxHostConfig::Local);
    }

    #[test]
    fn tmux_args_ssh_with_extra_args() {
        let (_, _, host) = parse_tmux_args(&words(
            "tmux work --ssh dev@box --ssh-arg -p --ssh-arg 2222",
        ))
        .unwrap();
        assert_eq!(
            host,
            TmuxHostConfig::Ssh {
                destination: "dev@box".into(),
                args: vec!["-p".into(), "2222".into()],
            }
        );
    }

    #[test]
    fn tmux_args_ssh_arg_requires_ssh() {
        assert!(parse_tmux_args(&words("tmux work --ssh-arg -p")).is_err());
    }

    #[test]
    fn tmux_args_wsl() {
        let (_, _, host) = parse_tmux_args(&words(
            "tmux work --wsl --wsl-arg --distribution --wsl-arg Ubuntu",
        ))
        .unwrap();
        assert_eq!(
            host,
            TmuxHostConfig::Wsl {
                args: vec!["--distribution".into(), "Ubuntu".into()],
            }
        );
    }

    #[test]
    fn tmux_args_ssh_and_wsl_conflict() {
        assert!(parse_tmux_args(&words("tmux work --ssh dev@box --wsl")).is_err());
    }
}
