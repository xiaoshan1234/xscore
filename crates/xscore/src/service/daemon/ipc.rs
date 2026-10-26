//! Local daemon IPC.
//!
//! Unix builds use a socket in the user's runtime/config directory. Native
//! Windows builds use a named pipe derived from the same per-user path, so the
//! wire protocol and the daemon state machine stay identical across platforms.

use std::io;
use std::path::Path;

use tokio::io::{AsyncRead, AsyncWrite};

/// A connection owned by the daemon protocol. Both Unix sockets and Windows
/// named pipes implement the required Tokio traits.
pub trait DaemonStream: AsyncRead + AsyncWrite + Unpin + Send {}

impl<T> DaemonStream for T where T: AsyncRead + AsyncWrite + Unpin + Send {}

pub type BoxDaemonStream = Box<dyn DaemonStream>;

#[cfg(windows)]
fn pipe_name(path: &Path) -> String {
    // FNV-1a keeps this independent of Rust's randomly seeded `DefaultHasher`
    // and short enough for the Windows named-pipe namespace. Hashing the
    // per-user config path isolates users and test HOME directories.
    let mut hash = 0xcbf29ce484222325u64;
    for byte in path.to_string_lossy().as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!(r"\\.\pipe\xscore-{hash:016x}")
}

#[cfg(unix)]
pub async fn connect(path: &Path) -> io::Result<BoxDaemonStream> {
    let stream = tokio::net::UnixStream::connect(path).await?;
    Ok(Box::new(stream))
}

#[cfg(windows)]
pub async fn connect(path: &Path) -> io::Result<BoxDaemonStream> {
    use tokio::net::windows::named_pipe::ClientOptions;

    const ERROR_PIPE_BUSY: i32 = 231;
    let name = pipe_name(path);
    for attempt in 0..10 {
        match ClientOptions::new().open(&name) {
            Ok(stream) => return Ok(Box::new(stream)),
            Err(e) if e.raw_os_error() == Some(ERROR_PIPE_BUSY) && attempt < 9 => {
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            }
            Err(e) => return Err(e),
        }
    }
    unreachable!("the retry loop either returns a stream or an error")
}

pub async fn probe(path: &Path) -> bool {
    connect(path).await.is_ok()
}

#[cfg(windows)]
pub fn display_address(path: &Path) -> String {
    pipe_name(path)
}

#[cfg(unix)]
pub fn display_address(path: &Path) -> String {
    path.display().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(windows)]
    #[test]
    fn named_pipe_name_is_stable_and_path_specific() {
        let a = pipe_name(Path::new(r"C:\Users\a\.config\xscore\run\daemon.sock"));
        let b = pipe_name(Path::new(r"C:\Users\a\.config\xscore\run\daemon.sock"));
        let c = pipe_name(Path::new(r"C:\Users\b\.config\xscore\run\daemon.sock"));
        assert_eq!(a, b);
        assert_ne!(a, c);
        assert!(a.starts_with(r"\\.\pipe\xscore-"));
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn named_pipe_roundtrips_the_daemon_frame_protocol() {
        use super::super::protocol::{read_frame, write_frame, Request, Response};
        use tokio::net::windows::named_pipe::{PipeMode, ServerOptions};

        let home = tempfile::tempdir().expect("temp HOME");
        let path = home.path().join("daemon.sock");
        let name = pipe_name(&path);
        let mut server = ServerOptions::new()
            .pipe_mode(PipeMode::Byte)
            .first_pipe_instance(true)
            .create(&name)
            .expect("create pipe");

        let connect = connect(&path);
        let (client, connected) = tokio::join!(connect, server.connect());
        let mut client = client.expect("connect client");
        connected.expect("connect server");

        write_frame(&mut *client, &Request::Ping)
            .await
            .expect("write request");
        let request: Request = read_frame(&mut server)
            .await
            .expect("read request")
            .expect("request frame");
        assert_eq!(request, Request::Ping);

        write_frame(&mut server, &Response::ok())
            .await
            .expect("write response");
        let response: Response = read_frame(&mut *client)
            .await
            .expect("read response")
            .expect("response frame");
        assert_eq!(response, Response::ok());
    }
}
