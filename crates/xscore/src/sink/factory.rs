use crate::core::{LinkId, Result};
use crate::model::SinkConfig;
use crate::transport::Transport;
use super::{raw::RawSink, tmux::TmuxSink, Sink};

/// Build a `Box<dyn Sink>` for a new link. The sink holds a reference to
/// `link_id` so it can populate `WindowInfo.link_id` on `new_window`.
pub async fn sink_for(
    link_id: LinkId,
    config: &SinkConfig,
    transport_kind: &str,
    transport: &mut dyn Transport,
) -> Result<Box<dyn Sink>> {
    match config {
        SinkConfig::Tmux { mode, session_name } => {
            let resolved_mode = match mode.as_str() {
                "auto" => if transport_kind == "ssh" { "remote" } else { "local" },
                other => other,
            };
            if resolved_mode == "off" {
                return Ok(Box::new(RawSink));
            }
            let name = session_name.clone()
                .unwrap_or_else(|| format!("xscore-{link_id}"));
            let s = TmuxSink::create(link_id, resolved_mode, &name, transport).await?;
            Ok(Box::new(s))
        }
    }
}