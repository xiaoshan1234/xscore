//! daemon 服务端：持有 `App` 并处理 IPC 请求。

use std::sync::Arc;

use anyhow::{Context, Result};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::{Mutex, Notify};

use super::ipc;
use super::protocol::{self, Request, Response};
use crate::app::{dispatch, App};

#[cfg(unix)]
pub async fn run() -> Result<()> {
    use tokio::net::UnixListener;

    let path = ipc::address();
    if ipc::probe().await {
        tracing::info!(address = %path, "daemon already running");
        return Ok(());
    }
    let socket_path = std::path::Path::new(&path);
    if socket_path.exists() {
        let _ = std::fs::remove_file(socket_path);
    }
    let listener = UnixListener::bind(socket_path).context("bind daemon socket")?;
    tracing::info!(address = %path, "daemon listening");

    let app = Arc::new(Mutex::new(App::new()));
    let shutdown = Arc::new(Notify::new());
    loop {
        tokio::select! {
            accepted = listener.accept() => {
                match accepted {
                    Ok((stream, _)) => {
                        let app = app.clone();
                        let shutdown = shutdown.clone();
                        tokio::spawn(async move { serve_connection(stream, app, shutdown).await });
                    }
                    Err(error) => tracing::warn!(%error, "daemon accept failed"),
                }
            }
            _ = shutdown.notified() => break,
        }
    }
    let _ = std::fs::remove_file(ipc::address());
    Ok(())
}

#[cfg(windows)]
pub async fn run() -> Result<()> {
    use tokio::net::windows::named_pipe::{NamedPipeServer, PipeMode, ServerOptions};

    let name = ipc::address();
    if ipc::probe().await {
        tracing::info!(address = %name, "daemon already running");
        return Ok(());
    }
    let mut options = ServerOptions::new();
    options
        .pipe_mode(PipeMode::Byte)
        .max_instances(32)
        .first_pipe_instance(true);
    let mut server: NamedPipeServer = options.create(&name).context("create named pipe")?;
    tracing::info!(address = %name, "daemon listening");

    let app = Arc::new(Mutex::new(App::new()));
    let shutdown = Arc::new(Notify::new());
    loop {
        tokio::select! {
            connected = server.connect() => {
                if let Err(error) = connected {
                    tracing::warn!(%error, "daemon accept failed");
                    continue;
                }
                let stream = server;
                server = match options.create(&name) {
                    Ok(next) => next,
                    Err(error) => {
                        tracing::error!(%error, "daemon could not create next pipe");
                        anyhow::bail!("could not create next pipe: {error}");
                    }
                };
                let app = app.clone();
                let shutdown = shutdown.clone();
                tokio::spawn(async move { serve_connection(stream, app, shutdown).await });
            }
            _ = shutdown.notified() => break,
        }
    }
    Ok(())
}

#[cfg(all(not(unix), not(windows)))]
pub async fn run() -> Result<()> {
    anyhow::bail!("the daemon requires Unix sockets or Windows named pipes on this target")
}

async fn serve_connection<S>(mut stream: S, app: Arc<Mutex<App>>, shutdown: Arc<Notify>)
where
    S: AsyncRead + AsyncWrite + Unpin + Send,
{
    loop {
        let request: Request = match protocol::read_frame(&mut stream).await {
            Ok(Some(request)) => request,
            Ok(None) => return,
            Err(error) => {
                tracing::warn!(%error, "daemon bad frame");
                return;
            }
        };
        let stop = matches!(request, Request::Stop);
        let response = handle(&app, request).await;
        if protocol::write_frame(&mut stream, &response).await.is_err() {
            return;
        }
        if stop {
            shutdown.notify_one();
            return;
        }
    }
}

async fn handle(app: &Arc<Mutex<App>>, request: Request) -> Response {
    match request {
        Request::Ping => Response::ok("pong\n".to_string()),
        Request::Status => Response::ok(app.lock().await.status().into_string()),
        Request::Stop => Response::ok(String::new()),
        Request::Command { cmd } => match dispatch(app, cmd).await {
            Ok((_, output)) => Response::ok(output.into_string()),
            Err(error) => Response::fail(format!("{error:#}")),
        },
    }
}
