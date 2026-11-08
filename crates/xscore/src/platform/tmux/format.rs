//! tmux 的两类文本接口：`-F` 输出解析与 `send-keys` 命令行。
//!
//! 注意：tmux 会把 `-F` 格式里的控制字符转义——tab 展开成 `ESC[NC`，其余 C0
//! 控制字节变成 `\ooo`。所以字段分隔符用可打印的 `|`，tmux 会原样输出。

use std::fmt::Write as _;

use super::{TmuxHostConfig, TmuxSocket};
use crate::{Error, Result};

/// `list-windows -F` 使用的字段格式。
pub const LIST_WINDOWS_FORMAT: &str =
    "#{window_id}|#{window_name}|#{window_width}|#{window_height}|#{window_layout}|#{window_active}";
/// `list-panes -F` 使用的字段格式。
pub const LIST_PANES_FORMAT: &str =
    "#{pane_id}|#{pane_width}|#{pane_height}|#{pane_left}|#{pane_top}";

/// 解析后的一行 tmux window 信息。
#[derive(Debug, Clone, PartialEq)]
pub struct WindowLine {
    /// tmux window 句柄。
    pub handle: String,
    /// window 名称。
    pub name: String,
    /// window 列数。
    pub cols: u16,
    /// window 行数。
    pub rows: u16,
    /// tmux 布局字符串。
    pub layout: String,
    /// 是否为当前活动 window。
    pub active: bool,
}

/// 解析后的一行 tmux pane 信息。
#[derive(Debug, Clone, PartialEq)]
pub struct PaneLine {
    /// tmux pane 句柄。
    pub handle: String,
    /// pane 列数。
    pub cols: u16,
    /// pane 行数。
    pub rows: u16,
    /// pane 在 window 中的左偏移。
    pub x: u16,
    /// pane 在 window 中的上偏移。
    pub y: u16,
}

/// 解析 `list-windows -F` 输出的一行。
///
/// 参数/输出：`line` 为输入；格式合法时返回 [`WindowLine`]，否则返回 `None`。
pub fn parse_window_line(line: &str) -> Option<WindowLine> {
    let fields: Vec<&str> = line.split('|').collect();
    if fields.len() != 6 {
        return None;
    }
    let handle = fields[0];
    if !handle.starts_with('@') {
        return None;
    }
    Some(WindowLine {
        handle: handle.to_string(),
        name: fields[1].to_string(),
        cols: fields[2].parse().ok()?,
        rows: fields[3].parse().ok()?,
        layout: fields[4].to_string(),
        active: fields[5].trim() == "1",
    })
}

/// 解析 `list-panes -F` 输出的一行。
///
/// 参数/输出：`line` 为输入；格式合法时返回 [`PaneLine`]，否则返回 `None`。
pub fn parse_pane_line(line: &str) -> Option<PaneLine> {
    let fields: Vec<&str> = line.split('|').collect();
    if fields.len() != 5 {
        return None;
    }
    let handle = fields[0];
    if !handle.starts_with('%') {
        return None;
    }
    Some(PaneLine {
        handle: handle.to_string(),
        cols: fields[1].parse().ok()?,
        rows: fields[2].parse().ok()?,
        x: fields[3].parse().ok()?,
        y: fields[4].parse().ok()?,
    })
}

/// 为 tmux 自身的命令解析器给单个参数加引号（控制通道上没有 shell）。
///
/// 参数/输出：`text` 为输入；返回可直接拼入 tmux 命令的单引号字符串。
pub fn quote_arg(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('\'');
    for ch in text.chars() {
        if ch == '\'' {
            out.push_str(r"'\''");
        } else {
            out.push(ch);
        }
    }
    out.push('\'');
    out
}

/// 把 `payload` 逐字投递给 pane `handle`：无换行走 `-l -- '...'`，有换行走
/// `-H <hex>`。NUL 无线路表示，拒绝。
///
/// 参数/输出：`handle` 和 `payload` 为输入；返回 tmux `send-keys` 命令，NUL
/// 输入返回错误。
pub fn send_keys_literal(handle: &str, payload: &str) -> Result<String> {
    if payload.contains('\0') {
        return Err(Error::Invalid(
            "tmux control mode cannot carry a NUL byte".into(),
        ));
    }
    if payload.contains('\n') || payload.contains('\r') {
        let mut command = format!("send-keys -t {handle} -H");
        for byte in payload.as_bytes() {
            let _ = write!(command, " {byte:02x}");
        }
        return Ok(command);
    }
    Ok(format!(
        "send-keys -t {handle} -l -- {}",
        quote_arg(payload)
    ))
}

/// 为本地进程型 tmux 宿主构造启动命令前缀。
///
/// 参数/输出：`host` 为输入；本地或 WSL 返回程序参数前缀，SSH 返回 `None`，
/// 因为 SSH tmux 由 russh channel 直接承载。
pub fn process_prefix(host: &TmuxHostConfig) -> Option<Vec<String>> {
    match host {
        TmuxHostConfig::Local => Some(Vec::new()),
        TmuxHostConfig::Ssh { .. } => None,
        TmuxHostConfig::Wsl { args } => {
            let mut argv = vec!["wsl.exe".to_string()];
            argv.extend(args.iter().cloned());
            argv.push("--exec".to_string());
            Some(argv)
        }
    }
}

/// 构造列出 session 内 window 的 tmux 控制命令。
///
/// 参数/输出：`session` 为输入；返回完整命令字符串。
pub fn list_windows_command(session: &str) -> String {
    format!(
        "list-windows -t {} -F {}",
        quote_arg(session),
        quote_arg(LIST_WINDOWS_FORMAT)
    )
}

/// 构造列出指定 window 内 pane 的 tmux 控制命令。
///
/// 参数/输出：`window_handle` 为输入；返回完整命令字符串。
pub fn list_panes_command(window_handle: &str) -> String {
    format!(
        "list-panes -t {} -F {}",
        quote_arg(window_handle),
        quote_arg(LIST_PANES_FORMAT)
    )
}

/// 构造调整单个 tmux pane 尺寸的命令。
///
/// 参数/输出：`handle`、`cols` 和 `rows` 为输入；返回完整命令字符串。
pub fn resize_pane_command(handle: &str, cols: u16, rows: u16) -> String {
    format!("resize-pane -t {handle} -x {cols} -y {rows}")
}

/// 把 socket 选择转换为 tmux 命令行参数。
///
/// 参数/输出：`socket` 为输入；返回空参数、`-L <name>` 或 `-S <path>`。
pub fn socket_args(socket: &TmuxSocket) -> Vec<String> {
    match socket {
        TmuxSocket::Default => Vec::new(),
        TmuxSocket::Named(name) => vec!["-L".to_string(), name.clone()],
        TmuxSocket::Path(path) => vec!["-S".to_string(), path.clone()],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn window_and_pane_lines_parse() {
        let window = parse_window_line("@3|vim|100|30|b67d,100x30,0,0,0|1").unwrap();
        assert_eq!(window.handle, "@3");
        assert_eq!((window.cols, window.rows), (100, 30));

        let pane = parse_pane_line("%0|100|23|0|1").unwrap();
        assert_eq!(pane.handle, "%0");
        assert_eq!((pane.cols, pane.rows), (100, 23));
        assert!(parse_window_line("3|vim|100|30|b67d|1").is_none());
        assert!(parse_pane_line("%0|100|23|0").is_none());
    }

    #[test]
    fn send_keys_quotes_or_hexes() {
        assert_eq!(
            send_keys_literal("%7", r#"a;b it's "q" $X 中文"#).unwrap(),
            r#"send-keys -t %7 -l -- 'a;b it'\''s "q" $X 中文'"#
        );
        assert_eq!(
            send_keys_literal("%0", "hello\nworld").unwrap(),
            "send-keys -t %0 -H 68 65 6c 6c 6f 0a 77 6f 72 6c 64"
        );
        assert!(send_keys_literal("%0", "a\0b").is_err());
    }

    #[test]
    fn socket_args_map_one_to_one() {
        assert_eq!(socket_args(&TmuxSocket::Default), Vec::<String>::new());
        assert_eq!(
            socket_args(&TmuxSocket::Named("x".into())),
            vec!["-L".to_string(), "x".to_string()]
        );
    }

    #[test]
    fn process_prefix_wsl_execs_tmux() {
        let host = TmuxHostConfig::Wsl {
            args: vec!["--distribution".into(), "Ubuntu".into()],
        };
        assert_eq!(
            process_prefix(&host),
            Some(vec![
                "wsl.exe".into(),
                "--distribution".into(),
                "Ubuntu".into(),
                "--exec".into()
            ])
        );
    }

    #[test]
    fn process_prefix_rejects_ssh_tmux() {
        let host = TmuxHostConfig::Ssh {
            user: "dev".into(),
            host: "box".into(),
            port: 2222,
            identity: None,
            password: None,
            options: crate::SshOptions::default(),
        };
        assert_eq!(process_prefix(&host), None);
    }
}
