//! 本地 IPC 端点：127.0.0.1 上的 TCP 回环端口。
//!
//! 用 TCP 而不是命名管道：回环 TCP 在所有平台一致、跨进程可靠，且便于测试。

use std::io;

use tokio::io::{AsyncRead, AsyncWrite};

pub trait DaemonStream: AsyncRead + AsyncWrite + Unpin + Send {}

impl<T> DaemonStream for T where T: AsyncRead + AsyncWrite + Unpin + Send {}

pub type BoxDaemonStream = Box<dyn DaemonStream>;

/// 由用户目录稳定派生出的回环端口，同一用户始终指向同一 daemon。
pub fn port() -> u16 {
    #[cfg(windows)]
    let identity = std::env::var("USERPROFILE").unwrap_or_default();
    #[cfg(not(windows))]
    let identity = std::env::var("HOME").unwrap_or_default();
    let hash = fnv1a(identity.as_bytes());
    40_000 + (hash % 20_000) as u16
}

fn fnv1a(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

pub async fn connect() -> io::Result<BoxDaemonStream> {
    let stream = tokio::net::TcpStream::connect(("127.0.0.1", port())).await?;
    stream.set_nodelay(true)?;
    Ok(Box::new(stream))
}

pub async fn probe() -> bool {
    connect().await.is_ok()
}

#[cfg(test)]
mod tests {
    use tokio::net::TcpListener;

    #[tokio::test]
    async fn tcp_roundtrips_the_frame_protocol() {
        use super::super::protocol::{self, Request, Response};

        let listener = TcpListener::bind(("127.0.0.1", 0)).await.expect("bind");
        let addr = listener.local_addr().expect("local addr");

        let connect = tokio::net::TcpStream::connect(addr);
        let (client, accepted) = tokio::join!(connect, listener.accept());
        let mut client = client.expect("connect client");
        let (mut server, _) = accepted.expect("accept server");

        protocol::write_frame(&mut client, &Request::Ping)
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
        let response: Response = protocol::read_frame(&mut client)
            .await
            .expect("read response")
            .expect("response frame");
        assert!(response.ok);
        assert_eq!(response.output, "pong\n");
    }
}
