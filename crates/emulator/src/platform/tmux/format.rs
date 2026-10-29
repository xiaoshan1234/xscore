//! tmux 的两类文本接口：`-F` 输出解析与 `send-keys` 命令行。
//!
//! 注意：tmux 会把 `-F` 格式里的控制字符转义——tab 展开成 `ESC[NC`，其余 C0
//! 控制字节变成 `\ooo`。所以字段分隔符用可打印的 `|`，tmux 会原样输出。

use std::fmt::Write as _;

use super::{TmuxHostConfig, TmuxSocket};
use crate::{Error, Result};

pub const LIST_WINDOWS_FORMAT: &str =
    "#{window_id}|#{window_name}|#{window_width}|#{window_height}|#{window_layout}|#{window_active}";
pub const LIST_PANES_FORMAT: &str =
    "#{pane_id}|#{pane_width}|#{pane_height}|#{pane_left}|#{pane_top}";

#[derive(Debug, Clone, PartialEq)]
pub struct WindowLine {
    pub handle: String,
    pub name: String,
    pub cols: u16,
    pub rows: u16,
    pub layout: String,
    pub active: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PaneLine {
    pub handle: String,
    pub cols: u16,
    pub rows: u16,
    pub x: u16,
    pub y: u16,
}

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

pub fn host_prefix(host: &TmuxHostConfig) -> Vec<String> {
    match host {
        TmuxHostConfig::Local => Vec::new(),
        TmuxHostConfig::Ssh {
            destination,
            args,
            password,
            ..
        } => {
            let mut argv: Vec<String> = vec!["ssh".into()];
            if password.is_none() {
                argv.extend(["-o".into(), "BatchMode=yes".into()]);
            }
            argv.extend(["-o".into(), "StrictHostKeyChecking=accept-new".into()]);
            argv.extend(args.iter().cloned());
            argv.push(destination.clone());
            argv
        }
        TmuxHostConfig::Wsl { args } => {
            let mut argv = vec!["wsl.exe".to_string()];
            argv.extend(args.iter().cloned());
            argv.push("--exec".to_string());
            argv
        }
    }
}

pub fn list_windows_command(session: &str) -> String {
    format!(
        "list-windows -t {} -F {}",
        quote_arg(session),
        quote_arg(LIST_WINDOWS_FORMAT)
    )
}

pub fn list_panes_command(window_handle: &str) -> String {
    format!(
        "list-panes -t {} -F {}",
        quote_arg(window_handle),
        quote_arg(LIST_PANES_FORMAT)
    )
}

pub fn resize_pane_command(handle: &str, cols: u16, rows: u16) -> String {
    format!("resize-pane -t {handle} -x {cols} -y {rows}")
}

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
    fn host_prefix_wsl_execs_tmux() {
        let host = TmuxHostConfig::Wsl {
            args: vec!["--distribution".into(), "Ubuntu".into()],
        };
        assert_eq!(
            host_prefix(&host),
            ["wsl.exe", "--distribution", "Ubuntu", "--exec"]
        );
    }

    #[test]
    fn host_prefix_ssh_accepts_new_host_keys() {
        let host = TmuxHostConfig::Ssh {
            destination: "dev@box".into(),
            args: vec!["-p".into(), "2222".into()],
            password: None,
            askpass: None,
        };
        assert_eq!(
            host_prefix(&host),
            [
                "ssh",
                "-o",
                "BatchMode=yes",
                "-o",
                "StrictHostKeyChecking=accept-new",
                "-p",
                "2222",
                "dev@box"
            ]
        );
    }
}
