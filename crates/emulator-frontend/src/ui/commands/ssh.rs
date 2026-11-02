//! SSH pane 生命周期。

use anyhow::Result;

use emulator::domain::{Domain, PaneConfig, RemoteSshDomain};

use crate::cli::parse_ssh_target;
use crate::ui::app::App;
use crate::ui::render::Output;

pub async fn ssh(
    app: &mut App,
    target: String,
    identity: Option<String>,
    password: Option<String>,
) -> Result<Output> {
    let (user, host, port) = parse_ssh_target(&target)?;
    let cols = 80u16;
    let rows = 24u16;
    let domain = RemoteSshDomain::new();
    let handle = domain
        .spawn_pane(&PaneConfig::Ssh {
            host,
            port,
            user,
            identity,
            password,
            cols,
            rows,
        })
        .await?;
    let id = app.register_pane(handle, cols, rows);
    app.layout.add(id);
    let mut out = Output::new();
    out.line(format!("pane: {id}  kind: ssh  {cols}x{rows}"));
    Ok(out)
}
