use crate::core::{LinkId, Result};
use crate::model::SinkConfig;
use crate::transport::Transport;
use super::{raw::RawSink, tmux::TmuxSink, Sink};

/// Build a `Box<dyn Sink>` for a new link.
///
/// For `RawSink` (the default), the transport is the I/O source. We
/// call `transport.open()` here so the bridge can read/write without
/// having to do it explicitly. For tmux links, `TmuxSink::create_link`
/// runs `tmux new-session -d` (which itself opens the transport) — so
/// we DON'T open the transport here for tmux (avoid the double-open
/// race; tmux already coordinated the open).
pub async fn sink_for(
    link_id: LinkId,
    config: &SinkConfig,
    transport_kind: &str,
    transport: &mut dyn Transport,
) -> Result<Box<dyn Sink>> {
    match config {
        SinkConfig::Raw => {
            // Open the transport so the bridge's read/write succeed
            // (raw transports like `ShellTransport` require an explicit
            // open before read/write).
            transport.open().await?;
            let _ = (link_id, transport_kind);
            Ok(Box::new(RawSink))
        }
        SinkConfig::Tmux { mode, session_name } => {
            let resolved_mode = match mode.as_str() {
                "auto" => if transport_kind == "ssh" { "remote" } else { "local" },
                other => other,
            };
            if resolved_mode == "off" {
                return Ok(Box::new(RawSink) as Box<dyn Sink>);
            }
            let name = session_name.clone()
                .unwrap_or_else(|| format!("xscore-{link_id}"));
            let s = TmuxSink::create_link(link_id, resolved_mode, &name, transport).await?;
            Ok(Box::new(s))
        }
    }
}