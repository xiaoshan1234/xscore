use crate::core::Result;
use crate::model::SinkConfig;
use crate::transport::Transport;
use super::{raw::RawSink, tmux::TmuxSink, Sink};

pub async fn sink_for(
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
                .unwrap_or_else(|| format!("xscore-{}", uuid::Uuid::now_v7()));
            let s = TmuxSink::create(resolved_mode, &name, transport).await?;
            Ok(Box::new(s))
        }
    }
}