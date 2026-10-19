use crate::core::{LinkId, Result};
use crate::model::SinkConfig;
use crate::transport::Transport;
use super::{raw::RawSink, tmux::TmuxSink, Sink};

/// Build a `Box<dyn Sink>` for a new link. The sink holds a reference to
/// `link_id` so it can populate `WindowInfo.link_id` on `new_window`.
///
/// For `RawSink` (the default), we don't take ownership of the
/// transport — the user-facing `data read`/`write`/`resize` ops go
/// through the transport directly. The transport stays owned by the
/// `Link` struct, and the `RawSink` is just a marker that says
/// "this link has no tmux session".
pub async fn sink_for(
    link_id: LinkId,
    config: &SinkConfig,
    transport_kind: &str,
    transport: &mut dyn Transport,
) -> Result<Box<dyn Sink>> {
    match config {
        // RawSink: direct transport I/O. We need a transport to hand
        // to RawSink so the `data write` path can pass it through
        // (the transport is still owned by Link, and we share a
        // reference via the manager). For now, we use a temporary
        // re-borrow; the Link's transport is what gets used at
        // runtime via the manager's call path.
        SinkConfig::Raw => {
            // RawSink currently doesn't store the transport (it has no
            // I/O of its own — the manager's `backend.write` /
            // `backend.snapshot` go directly to `link.transport` via
            // the CompositeBackend in backends/composite.rs). So we
            // can construct a unit RawSink and drop the transport
            // borrow here.
            let _ = (link_id, transport_kind, transport); // suppress unused
            Ok(Box::new(RawSink))
        }
        SinkConfig::Tmux { mode, session_name } => {
            let resolved_mode = match mode.as_str() {
                "auto" => if transport_kind == "ssh" { "remote" } else { "local" },
                other => other,
            };
            if resolved_mode == "off" {
                // Defensive: shouldn't happen with the new URL parser
                // (it produces SinkConfig::Raw for "off"), but keep
                // the path for state files that have a Tmux variant
                // with mode="off" (e.g. user-edited state).
                return Ok(Box::new(RawSink));
            }
            let name = session_name.clone()
                .unwrap_or_else(|| format!("xscore-{link_id}"));
            let s = TmuxSink::create(link_id, resolved_mode, &name, transport).await?;
            Ok(Box::new(s))
        }
    }
}
