pub const DEFAULT_SOCKET: &str = "xscore";
pub fn socket_name() -> String {
    std::env::var("XSCORE_SOCKET").unwrap_or_else(|_| DEFAULT_SOCKET.to_string())
}
pub fn tmux_arg_socket() -> Vec<String> { vec!["-L".into(), socket_name()] }