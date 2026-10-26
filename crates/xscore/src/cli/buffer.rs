use std::sync::Arc;

use clap::{Args, Subcommand};

use crate::core::{BufferId, SessionManager};

#[derive(Subcommand)]
pub enum BufferCmd {
    /// List independent buffers, optionally for one session.
    List(BufferListArgs),
    /// Print BufferInfo JSON.
    Show {
        #[arg(long)]
        id: String,
    },
    /// Delete a buffer and stop its backend route. UI panes become empty.
    Del {
        #[arg(long)]
        id: String,
    },
}

#[derive(Args)]
pub struct BufferListArgs {
    #[arg(long, value_name = "NAME")]
    pub session: Option<String>,
}

pub async fn run(mgr: Arc<SessionManager>, cmd: BufferCmd) -> anyhow::Result<()> {
    match cmd {
        BufferCmd::List(args) => {
            let session = match args.session.as_deref() {
                Some(name) => Some(mgr.resolve_session(Some(name)).await?),
                None => None,
            };
            println!(
                "{:<36}  {:<36}  {:<20}",
                "BUFFER ID", "SESSION ID", "ORIGIN"
            );
            let mut buffers = mgr.buffers.list_all().await;
            buffers.sort_by_key(|buffer| buffer.created_at);
            for buffer in buffers {
                if session.is_some_and(|session| buffer.session_id != session) {
                    continue;
                }
                let origin = match &buffer.origin {
                    crate::model::BufferOrigin::Link { .. } => "raw-link",
                    crate::model::BufferOrigin::Tmux { .. } => "tmux-pane",
                };
                println!(
                    "{:<36}  {:<36}  {:<20}",
                    buffer.id, buffer.session_id, origin
                );
            }
            Ok(())
        }
        BufferCmd::Show { id } => {
            let id = BufferId::parse(&id)?;
            let buffer = mgr
                .buffers
                .get(id)
                .await
                .ok_or_else(|| anyhow::anyhow!("buffer not found: {id}"))?;
            println!("{}", serde_json::to_string_pretty(&buffer)?);
            Ok(())
        }
        BufferCmd::Del { id } => {
            let id = BufferId::parse(&id)?;
            mgr.del_buffer(id).await?;
            println!("deleted buffer: {id}");
            Ok(())
        }
    }
}
