// cli/data.rs
//
// `xscore data {read, write, resize}` — task-11 territory. This stub
// declares the subcommand + variant so the parent `cli/mod.rs` can do
// `pub mod data;` and the Commands::Data variant can be matched. The
// actual `read`/`write`/`resize` implementations land in task-11 (which
// also adds a `data.rs` spec file — not yet written).

use clap::Subcommand;

#[derive(Subcommand)]
pub enum DataCmd {
    /// Read recent bytes from a pane (task-11)
    Read {
        #[arg(long)] pane: String,
        #[arg(long, default_value_t = 4096)] bytes: usize,
    },
    /// Write bytes to a pane (task-11)
    Write {
        #[arg(long)] pane: String,
        /// Source file (use `-` for stdin)
        #[arg(long)] from_file: Option<String>,
    },
    /// Resize a pane (task-11)
    Resize {
        #[arg(long)] pane: String,
        #[arg(long)] cols: u16,
        #[arg(long)] rows: u16,
    },
}

pub fn run(_cmd: DataCmd) -> anyhow::Result<()> {
    anyhow::bail!("xscore data: not yet implemented (task-11)")
}