//! 本地 IPC 端点：Unix socket / Windows 命名管道。

use std::io;

use tokio::io::{AsyncRead, AsyncWrite};

pub trait DaemonStream: AsyncRead + AsyncWrite + Unpin + Send {}

impl<T> DaemonStream for T where T: AsyncRead + AsyncWrite + Unpin + Send {}

pub type BoxDaemonStream = Box<dyn DaemonStream>;

/// 端点的稳定名字：同一用户始终指向同一 daemon。
pub fn address() -> String {
    #[cfg(windows)]
    let identity = std::env::var("USERPROFILE").unwrap_or_default();
    #[cfg(not(windows))]
    let identity = std::env::var("HOME").unwrap_or_default();
    let hash = fnv1a(identity.as_bytes());
    #[cfg(windows)]
    {
        format!(r"\\.\pipe\xscore-{hash:016x}")
    }
    #[cfg(not(windows))]
    {
        std::env::temp_dir()
            .join(format!("xscore-{hash:016x}.sock"))
            .to_string_lossy()
            .into_owned()
    }
}

fn fnv1a(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

#[cfg(unix)]
pub async fn connect() -> io::Result<BoxDaemonStream> {
    connect_to(&address()).await
}

#[cfg(unix)]
pub async fn connect_to(path: &str) -> io::Result<BoxDaemonStream> {
    let stream = tokio::net::UnixStream::connect(path).await?;
    Ok(Box::new(stream))
}

#[cfg(windows)]
pub async fn connect() -> io::Result<BoxDaemonStream> {
    connect_to(&address()).await
}

#[cfg(windows)]
pub async fn connect_to(name: &str) -> io::Result<BoxDaemonStream> {
    use tokio::net::windows::named_pipe::ClientOptions;

    const ERROR_PIPE_BUSY: i32 = 231;
    for attempt in 0..10 {
        match ClientOptions::new().open(name) {
            Ok(stream) => return Ok(Box::new(stream)),
            Err(e) if e.raw_os_error() == Some(ERROR_PIPE_BUSY) && attempt < 9 => {
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            }
            Err(e) => return Err(e),
        }
    }
    unreachable!("the retry loop either returns a stream or an error")
}

pub async fn probe() -> bool {
    connect().await.is_ok()
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use tokio::net::windows::named_pipe::{PipeMode, ServerOptions};

    #[tokio::test]
    async fn named_pipe_roundtrips_the_frame_protocol() {
        use super::super::protocol::{self, Request, Response};

        let name = format!(r"\\.\pipe\xscore-test-{}", std::process::id());
        let mut server = ServerOptions::new()
            .pipe_mode(PipeMode::Byte)
            .first_pipe_instance(true)
            .create(&name)
            .expect("create test pipe");

        let connect = connect_to(&name);
        let (client, connected) = tokio::join!(connect, server.connect());
        let mut client = client.expect("connect client");
        connected.expect("connect server");

        protocol::write_frame(&mut *client, &Request::Ping)
            .await
            .expect("write request");
        let request: Request = protocol::read_frame(&mut server)
            .await
            .expect("read request")
            .expect("request frame");
        assert!(matches!(request, Request::Ping));

        protocol::write_frame(&mut server, &Response::ok("pong\n".into()))
            .await
            .expect("write response");
        let response: Response = protocol::read_frame(&mut *client)
            .await
            .expect("read response")
            .expect("response frame");
        assert!(response.ok);
        assert_eq!(response.output, "pong\n");
    }
}
