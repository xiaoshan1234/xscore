# Task 11: CLI `data` subcommand — read/write/resize

Per-pane data ops (the only place pane_id is needed).

**Files:**
- Modify: `crates/xscore/src/cli/data.rs`

```rust
// cli/data.rs
use crate::core::PaneId;
use crate::core::SessionManager;
use clap::Subcommand;
use std::sync::Arc;

#[derive(Subcommand)]
pub enum DataCmd {
    Read { #[arg(long)] pane: String, #[arg(long, default_value_t = 4096)] bytes: usize },
    Write { #[arg(long)] pane: String, #[arg(long)] from_file: Option<String> },
    Resize { #[arg(long)] pane: String, #[arg(long)] cols: u16, #[arg(long)] rows: u16 },
}

pub async fn run(mgr: Arc<SessionManager>, cmd: DataCmd) -> anyhow::Result<()> {
    match cmd {
        DataCmd::Read { pane, bytes } => {
            let pid = PaneId::parse(&pane)?;
            let backend = mgr.backend(pid).await?;
            let mut buf = vec![0u8; bytes];
            let n = backend.snapshot(&mut buf).await?;
            std::io::Write::write_all(&mut std::io::stdout(), &buf[..n])?;
        }
        DataCmd::Write { pane, from_file } => {
            let pid = PaneId::parse(&pane)?;
            let backend = mgr.backend(pid).await?;
            let data = if let Some(p) = from_file { std::fs::read(p)? } else {
                let mut s = String::new();
                std::io::Read::read_to_string(&mut std::io::stdin(), &mut s)?;
                s.into_bytes()
            };
            let n = backend.write(&data).await?;
            eprintln!("wrote {n} bytes");
        }
        DataCmd::Resize { pane, cols, rows } => {
            let pid = PaneId::parse(&pane)?;
            let backend = mgr.backend(pid).await?;
            backend.resize(cols, rows).await?;
            eprintln!("resized to {cols}x{rows}");
        }
    }
    Ok(())
}
```

**Step 2: Wire to main**

```rust
match cli.command {
    // ...
    xscore::cli::Commands::Data(c) => xscore::cli::data::run(mgr, c).await?,
    // ...
}
```

**Step 3: Commit**

```bash
git add crates/xscore/src/cli/data.rs crates/xscore/src/main.rs
git commit -m "feat(cli): data read/write/resize over pane-id"
```