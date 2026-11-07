//! Resident terminal emulator server.

use anyhow::Result;
use xsc_server::{logging, server};

#[tokio::main]
async fn main() -> Result<()> {
    if let Err(error) = logging::init() {
        eprintln!("warning: logging disabled: {error:#}");
    }
    server::run().await
}
