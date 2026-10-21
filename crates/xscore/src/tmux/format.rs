// tmux/format.rs — tmux's two text interfaces, in one place.
//
// Reading: the `-F` formats we ask tmux for, and the parser for the lines it
// answers with. Writing: the command line that delivers a payload to a pane.
//
// Both are text that tmux parses, and both have a measured gotcha that this
// module exists to keep in one place:
//
//   * a `\t` in a `-F` argument is NOT a tab. Measured on 3.5a:
//     `list-windows -F '#{window_id}\t#{window_name}'` answers with a literal
//     backslash-t (`b'@0\\tsh\\t100'`), so the format has to carry real tabs.
//     That is why the constants below use Rust's `\t`.
//   * a raw newline in a quoted `send-keys -l` payload ENDS the command line.
//     Measured on 3.5a: `send-keys -t %0 -l -- 'hello<LF>world'` answers
//     `parse error: unknown command: world`. Payloads with a newline therefore
//     go through `-H` (hex bytes), which is exact by construction.

use std::fmt::Write as _;

use crate::core::{Error, Result};
use crate::model::{TmuxHostConfig, TmuxSocket};

/// Fields of `list-windows`, in this order. Real tabs (see the module comment).
pub const LIST_WINDOWS_FORMAT: &str =
    "#{window_id}\t#{window_name}\t#{window_width}\t#{window_height}\t#{window_layout}\t#{window_active}";

/// Fields of `list-panes`, in this order.
pub const LIST_PANES_FORMAT: &str =
    "#{pane_id}\t#{pane_width}\t#{pane_height}\t#{pane_left}\t#{pane_top}";

/// One `list-windows` line.
#[derive(Debug, Clone, PartialEq)]
pub struct WindowLine {
    /// `@N` — identity, never the index (arch.md §8).
    pub handle: String,
    pub name: String,
    pub cols: u16,
    pub rows: u16,
    /// tmux's own layout string; geometry is read from it, never written.
    pub layout: String,
    pub active: bool,
}

/// One `list-panes` line.
#[derive(Debug, Clone, PartialEq)]
pub struct PaneLine {
    /// `%N` — this is the pane's `link_key` (ADR 0010 §D3).
    pub handle: String,
    pub cols: u16,
    pub rows: u16,
    pub x: u16,
    pub y: u16,
}

/// Parse a `list-windows` line. `None` for anything that is not one: a
/// malformed line is skipped, never fatal (a tmux that prints a warning on the
/// same stream must not corrupt the rows).
pub fn parse_window_line(line: &str) -> Option<WindowLine> {
    let fields: Vec<&str> = line.split('\t').collect();
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

/// Parse a `list-panes` line. Same policy as `parse_window_line`.
pub fn parse_pane_line(line: &str) -> Option<PaneLine> {
    let fields: Vec<&str> = line.split('\t').collect();
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

/// Quote one argument for tmux's *own* command parser (there is no shell on the
/// control channel, so this is the only layer; arch.md §6).
///
/// Inside single quotes tmux takes every byte literally — including `;`, `"`,
/// `$`, backticks and `\` — so the only character needing work is the quote
/// itself. Measured on 3.5a: `'a'\''b'` arrives as `a'b`, i.e. close, a
/// backslash-escaped quote, reopen.
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

/// The command line that delivers `payload` to pane `handle` **literally**.
///
/// Two forms, both measured on tmux 3.5a:
///
/// * no newline → `send-keys -t %N -l -- '<payload>'`. Measured with
///   `a;b it's "q" $X \ Z 中文 % #`: `capture-pane -p` gives exactly those
///   bytes back, so quoting is the only transformation and `'\''` survives
///   tmux's parser. `--` is accepted by 3.5a and keeps a payload that starts
///   with `-` from being read as an option.
/// * contains `\n`/`\r` → `send-keys -t %N -H <hex bytes>`. A raw newline is
///   the end of a tmux command line (measured: `parse error: unknown command`),
///   and tmux has no `\n` escape inside quotes (`\n` arrives as backslash-n),
///   so hex is the only exact form. Measured: `hello\nworld 中文 \e[1m` as
///   `68 65 …` arrives byte-for-byte.
///
/// `\0` is rejected: one protocol line is one record, and a NUL has no
/// representation in it (arch.md §6, "Two limits remain").
pub fn send_keys_literal(handle: &str, payload: &str) -> Result<String> {
    if payload.contains('\0') {
        return Err(Error::Invalid(
            "tmux control mode cannot carry a NUL byte (the protocol is line-framed)".into(),
        ));
    }
    if payload.contains('\n') || payload.contains('\r') {
        let mut command = format!("send-keys -t {handle} -H");
        for byte in payload.as_bytes() {
            // Writing into a String cannot fail.
            let _ = write!(command, " {byte:02x}");
        }
        return Ok(command);
    }
    Ok(format!("send-keys -t {handle} -l -- {}", quote_arg(payload)))
}

/// The command prefix in front of `tmux`: empty locally, one `ssh` segment
/// remotely (arch.md §6). `BatchMode=yes` is mandatory — a password prompt
/// would block a non-interactive CLI forever.
pub fn host_prefix(host: &TmuxHostConfig) -> Vec<String> {
    match host {
        TmuxHostConfig::Local => Vec::new(),
        TmuxHostConfig::Ssh { destination, args } => {
            let mut argv: Vec<String> = vec!["ssh".into(), "-o".into(), "BatchMode=yes".into()];
            argv.extend(args.iter().cloned());
            argv.push(destination.clone());
            argv
        }
    }
}

/// A one-shot `tmux <cmd>` argv (the other exec shape in arch.md §6: requests
/// that need no state, like `has-session` or `kill-session`).
pub fn one_shot_argv(host: &TmuxHostConfig, socket: &TmuxSocket, args: &[String]) -> Vec<String> {
    let mut argv = host_prefix(host);
    argv.push("tmux".into());
    argv.extend(crate::tmux::control::socket_args(socket));
    argv.extend(args.iter().cloned());
    argv
}

/// How a host is named in errors and listings.
pub fn host_label(host: &TmuxHostConfig) -> String {
    match host {
        TmuxHostConfig::Local => "local".to_string(),
        TmuxHostConfig::Ssh { destination, .. } => destination.clone(),
    }
}

/// `list-windows -t <session> -F …`, the enumeration every reconcile starts
/// from.
pub fn list_windows_command(session: &str) -> String {
    format!(
        "list-windows -t {} -F {}",
        quote_arg(session),
        quote_arg(LIST_WINDOWS_FORMAT)
    )
}

/// `list-panes -t <window> -F …` — one window's panes, by `@N` handle.
pub fn list_panes_command(window_handle: &str) -> String {
    format!(
        "list-panes -t {} -F {}",
        quote_arg(window_handle),
        quote_arg(LIST_PANES_FORMAT)
    )
}

/// `resize-pane -t %N -x <cols> -y <rows>`: the only way to resize a tmux pane
/// (arch.md §4, "resize is real where something owns a size").
pub fn resize_pane_command(handle: &str, cols: u16, rows: u16) -> String {
    format!("resize-pane -t {handle} -x {cols} -y {rows}")
}

/// `new-window -d -P -F '#{window_id}' [-n <name>]`: create a window in the
/// client's session and answer with its `@N`.
///
/// `-d` keeps the user's current window: creating a window must not move what
/// someone is looking at. `-P -F` is the only reliable way to learn the handle —
/// matching on names would confuse two windows that share one.
pub fn new_window_command(name: Option<&str>) -> String {
    let mut command = format!(
        "new-window -d -P -F {}",
        quote_arg("#{window_id}")
    );
    if let Some(name) = name.filter(|n| !n.is_empty()) {
        command.push_str(" -n ");
        command.push_str(&quote_arg(name));
    }
    command
}

/// `split-window -d -t @N -P -F '#{pane_id}'`: split one tmux window and answer
/// with the new pane's `%N` (its `link_key`).
pub fn split_window_command(window_handle: &str) -> String {
    format!(
        "split-window -d -t {} -P -F {}",
        quote_arg(window_handle),
        quote_arg("#{pane_id}")
    )
}

/// `kill-pane -t %N`: destroy a pane on the server. Used by `pane del`, because
/// dropping only the row would leave a `%N` that reconciliation adopts back.
pub fn kill_pane_command(handle: &str) -> String {
    format!("kill-pane -t {handle}")
}

/// `capture-pane -p -t %N`: tmux's own scrollback, the fallback a fresh process
/// reads when it streamed nothing itself (see `ControlBridge::read`).
pub fn capture_pane_command(handle: &str) -> String {
    format!("capture-pane -p -t {handle}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn window_lines_parse_and_malformed_ones_are_skipped() {
        let good = "@3\tvim\t100\t30\tb67d,100x30,0,0,0\t1";
        let parsed = parse_window_line(good).expect("good line must parse");
        assert_eq!(parsed.handle, "@3");
        assert_eq!(parsed.name, "vim");
        assert_eq!((parsed.cols, parsed.rows), (100, 30));
        assert_eq!(parsed.layout, "b67d,100x30,0,0,0");
        assert!(parsed.active);

        // A line with extra text (a warning, a stray echo) is dropped, not
        // half-adopted.
        assert!(parse_window_line("@3\tvim\t100\t30\tb67d\t1\textra").is_none());
        assert!(parse_window_line("list-windows: no server running").is_none());
        assert!(parse_window_line("").is_none());
        // Identity must be a handle, never an index.
        assert!(parse_window_line("3\tvim\t100\t30\tb67d\t1").is_none());
        // Geometry that is not a number is a malformed line, not a zero.
        assert!(parse_window_line("@3\tvim\twide\t30\tb67d\t1").is_none());
    }

    #[test]
    fn pane_lines_parse_and_malformed_ones_are_skipped() {
        let parsed = parse_pane_line("%0\t100\t23\t0\t1").expect("good line must parse");
        assert_eq!(parsed.handle, "%0");
        assert_eq!((parsed.cols, parsed.rows), (100, 23));
        assert_eq!((parsed.x, parsed.y), (0, 1));

        assert!(parse_pane_line("%0\t100\t23\t0").is_none());
        assert!(parse_pane_line("0\t100\t23\t0\t0").is_none());
        assert!(parse_pane_line("%0\t100\t23\t0\t0\t1").is_none());
        assert!(parse_pane_line("can't find window: @9").is_none());
    }

    #[test]
    fn the_formats_carry_real_tabs() {
        // Measured: `\t` in a -F argument reaches tmux as backslash-t, which
        // makes every line one field and every enum empty.
        assert_eq!(LIST_WINDOWS_FORMAT.matches('\t').count(), 5);
        assert_eq!(LIST_PANES_FORMAT.matches('\t').count(), 4);
        assert!(!LIST_WINDOWS_FORMAT.contains("\\t"), "a literal backslash-t would break parsing");
        assert!(list_windows_command("work").contains(LIST_WINDOWS_FORMAT));
    }

    #[test]
    fn send_keys_quotes_every_special_character() {
        let payload = r#"a;b it's "q" $X \ Z 中文 % #"#;
        let command = send_keys_literal("%7", payload).expect("no NUL");
        assert_eq!(
            command,
            r#"send-keys -t %7 -l -- 'a;b it'\''s "q" $X \ Z 中文 % #'"#
        );
        // Every quoted segment is closed exactly where a quote was opened:
        // an unbalanced form is a silent mis-type rather than an error.
        assert_eq!(command.matches('\'').count(), 5);
    }

    #[test]
    fn send_keys_uses_hex_when_the_payload_has_a_newline() {
        // A raw newline ends the tmux command line (measured), so this form
        // must be taken instead of quoting.
        let command = send_keys_literal("%0", "hello\nworld").expect("no NUL");
        assert_eq!(
            command,
            "send-keys -t %0 -H 68 65 6c 6c 6f 0a 77 6f 72 6c 64"
        );
        // ...and it is still one line, which is what the protocol requires.
        assert_eq!(command.lines().count(), 1);
        assert!(!command.contains('\n'));

        // `\r` gets the same treatment.
        assert_eq!(
            send_keys_literal("%1", "a\rb").unwrap(),
            "send-keys -t %1 -H 61 0d 62"
        );
    }

    #[test]
    fn nul_is_rejected_and_a_leading_dash_is_not_an_option() {
        match send_keys_literal("%0", "a\0b") {
            Err(Error::Invalid(_)) => {}
            other => panic!("expected Invalid for a NUL payload, got {other:?}"),
        }
        let command = send_keys_literal("%0", "-x --y").unwrap();
        assert_eq!(command, "send-keys -t %0 -l -- '-x --y'");
    }

    #[test]
    fn create_and_split_commands_ask_for_the_handle() {
        // `-P -F` is what makes the answer the handle; without it the row would
        // have to be guessed from a name or an index.
        let created = new_window_command(None);
        assert_eq!(created, "new-window -d -P -F '#{window_id}'");
        assert!(created.starts_with("new-window "));

        let named = new_window_command(Some("logs"));
        assert_eq!(
            named,
            "new-window -d -P -F '#{window_id}' -n 'logs'"
        );
        // An empty name is "no name", not `-n ''`.
        assert_eq!(new_window_command(Some("")), created);

        assert_eq!(
            split_window_command("@3"),
            "split-window -d -t '@3' -P -F '#{pane_id}'"
        );
        assert_eq!(kill_pane_command("%7"), "kill-pane -t %7");
    }

    #[test]
    fn host_prefix_is_empty_locally_and_one_ssh_segment_remotely() {
        assert!(host_prefix(&TmuxHostConfig::Local).is_empty());

        let host = TmuxHostConfig::Ssh {
            destination: "devbox".into(),
            args: vec!["-p".into(), "2222".into()],
        };
        assert_eq!(
            host_prefix(&host),
            ["ssh", "-o", "BatchMode=yes", "-p", "2222", "devbox"]
        );
        // The socket flag lands after `tmux`, on the right side of ssh.
        assert_eq!(
            one_shot_argv(&host, &TmuxSocket::Named("x".into()), &["has-session".into()]),
            ["ssh", "-o", "BatchMode=yes", "-p", "2222", "devbox", "tmux", "-L", "x", "has-session"]
        );
    }
}
