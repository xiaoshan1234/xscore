// util/tmux_socket.rs — which tmux server a command means.
//
// The default socket is `xscore`, and `XSCORE_SOCKET` overrides it (the test
// suite sets it per test so no test can ever touch the user's server). Since
// ADR 0010 a session stores its socket, so a *new* session has to resolve one
// at creation time: `--socket <name>` is the explicit answer, `XSCORE_SOCKET`
// the environment default, and only then the built-in name.
use crate::model::TmuxSocket;

pub const DEFAULT_SOCKET: &str = "xscore";

/// The socket name from the environment, else the default.
pub fn socket_name() -> String {
    std::env::var("XSCORE_SOCKET").unwrap_or_else(|_| DEFAULT_SOCKET.to_string())
}

/// `-L <name>` for the legacy one-shot tmux commands.
pub fn tmux_arg_socket() -> Vec<String> {
    vec!["-L".into(), socket_name()]
}

/// Resolve the socket for a **new** session.
///
/// Precedence, in this order: `--socket`, then `XSCORE_SOCKET`, then the
/// default server. `--socket` wins outright rather than being merged with the
/// environment: an explicit flag that the environment could still override
/// would make `--socket` unusable in exactly the case it exists for (a test or
/// a script that exports `XSCORE_SOCKET`).
pub fn resolve_socket(flag: Option<&str>) -> TmuxSocket {
    resolve_socket_from(flag, std::env::var("XSCORE_SOCKET").ok().as_deref())
}

/// `resolve_socket` with the environment passed in, so precedence is testable
/// without mutating the process environment.
pub fn resolve_socket_from(flag: Option<&str>, env: Option<&str>) -> TmuxSocket {
    if let Some(name) = flag.filter(|n| !n.is_empty()) {
        return TmuxSocket::Named(name.to_string());
    }
    match env.filter(|n| !n.is_empty()) {
        Some(name) => TmuxSocket::Named(name.to_string()),
        None => TmuxSocket::Default,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_flag_beats_the_environment() {
        // `--socket` wins even when the environment names another server: a
        // flag the environment can override is not a flag.
        assert_eq!(
            resolve_socket_from(Some("flag"), Some("env")),
            TmuxSocket::Named("flag".into())
        );
        assert_eq!(
            resolve_socket_from(Some("flag"), None),
            TmuxSocket::Named("flag".into())
        );
    }

    #[test]
    fn the_environment_wins_only_without_a_flag() {
        assert_eq!(
            resolve_socket_from(None, Some("xscore-test-x")),
            TmuxSocket::Named("xscore-test-x".into())
        );
        // An empty environment variable is not a socket name.
        assert_eq!(resolve_socket_from(None, Some("")), TmuxSocket::Default);
    }

    #[test]
    fn neither_means_the_default_server() {
        assert_eq!(resolve_socket_from(None, None), TmuxSocket::Default);
        // An empty flag is "not given", so the environment still applies.
        assert_eq!(
            resolve_socket_from(Some(""), Some("env")),
            TmuxSocket::Named("env".into())
        );
    }

    #[test]
    fn the_wrapper_reads_the_environment() {
        // The flag path is the one that must not depend on the environment.
        assert_eq!(
            resolve_socket(Some("explicit")),
            TmuxSocket::Named("explicit".into())
        );
        assert_eq!(DEFAULT_SOCKET, "xscore");
    }
}
