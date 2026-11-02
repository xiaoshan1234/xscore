//! daemon 服务端：持有 `App` 并处理 IPC 请求。

use std::sync::Arc;

use anyhow::{Context, Result};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::TcpListener;
use tokio::sync::{Mutex, Notify};

use super::ipc;
use super::protocol::{self, Request, Response};
use crate::ui::app::App;
use crate::ui::commands::dispatch;
use crate::ui::render::status_text;

pub async fn run() -> Result<()> {
    let port = ipc::port();
    if ipc::probe().await {
        tracing::info!(%port, "daemon already running");
        return Ok(());
    }
    let listener = match TcpListener::bind(("127.0.0.1", port)).await {
        Ok(listener) => listener,
        Err(error) => {
            // 两个客户端同时自动拉起时的竞态：另一个 daemon 可能刚绑定成功。
            if ipc::probe().await {
                return Ok(());
            }
            return Err(error).context("bind daemon port");
        }
    };
    tracing::info!(%port, "daemon listening");
    eprintln!("xs daemon listening on 127.0.0.1:{port} (stop with `xs daemon stop`)");

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
    Ok(())
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
        Request::Status => {
            let guard = app.lock().await;
            Response::ok(status_text(&guard).into_string())
        }
        Request::Stop => Response::ok(String::new()),
        Request::Command { cmd } => match dispatch(app, cmd).await {
            Ok(output) => Response::ok(output.into_string()),
            Err(error) => Response::fail(format!("{error:#}")),
        },
    }
}
