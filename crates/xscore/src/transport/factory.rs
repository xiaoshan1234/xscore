// transport/factory.rs — STUB (real impl pending task-07 + task-08).
//
// The canonical factory from task-06 Step 5 dispatches to
// `ShellTransport::new`, `SerialTransport::new`, `SshTransport::connect`.
// Those types live in transport/{shell,serial,ssh}.rs which are still
// 1-line placeholders until task-07 (shell) and task-08 (serial, ssh)
// land. Linking the real dispatch here now would break compilation in
// this slice.
//
// Once tasks 07/08 land, replace the body with the task-06 Step 5 match
// arm (verbatim). The signature stays the same.
use crate::core::Result;
use crate::model::TransportConfig;
use super::Transport;

#[allow(unused_variables)]
pub async fn transport_for(config: TransportConfig) -> Result<Box<dyn Transport>> {
    // Stub: full impl lands when task-07 (Shell) + task-08 (Serial, Ssh)
    // populate transport/{shell,serial,ssh}.rs with real Transport impls.
    todo!("transport_for: real impl pending task-07 + task-08")
}
