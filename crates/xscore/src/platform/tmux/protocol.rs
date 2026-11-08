//! `tmux -CC` control-mode line protocol: pure parsing, no I/O.

/// One line of the control-mode protocol, classified by prefix.
#[derive(Debug, Clone, PartialEq)]
pub enum Parsed {
    /// Start of a synchronous command reply block.
    Begin {
        /// tmux command number.
        num: u64,
        /// Flags carried by `%begin`.
        flags: u32,
    },
    /// End of a synchronous command reply block.
    End {
        /// tmux command number.
        num: u64,
    },
    /// A command reply block failed.
    Error {
        /// tmux command number.
        num: u64,
        /// tmux-provided error text.
        message: String,
    },
    /// Pane bytes. `pane` is the tmux handle (e.g. `%3`).
    Output {
        /// Handle of the pane that produced the output.
        pane: String,
        /// Decoded raw terminal bytes.
        bytes: Vec<u8>,
    },
    /// Historical pane bytes (`%extended-output`). `age_ms` is how long
    /// ago tmux generated the bytes.
    ExtendedOutput {
        /// Handle of the pane that produced the output.
        pane: String,
        /// Age of the output in milliseconds.
        age_ms: u64,
        /// Decoded raw terminal bytes.
        bytes: Vec<u8>,
    },
    /// A structural notification.
    Event(TmuxEvent),
    /// An unrecognised or incomplete control-mode record.
    Unknown(String),
}

/// Structural notifications from the tmux server.
#[derive(Debug, Clone, PartialEq)]
pub enum TmuxEvent {
    /// A window was added.
    WindowAdd {
        /// tmux window handle.
        handle: String,
    },
    /// A window was closed.
    WindowClose {
        /// tmux window handle.
        handle: String,
    },
    /// A window was renamed.
    WindowRenamed {
        /// tmux window handle.
        handle: String,
        /// New window name.
        name: String,
    },
    /// A window layout changed.
    LayoutChange {
        /// tmux window handle.
        handle: String,
        /// New tmux layout string.
        layout: String,
    },
    /// The session's active window changed.
    SessionWindowChanged {
        /// tmux session handle.
        session: String,
        /// tmux window handle.
        window: String,
    },
    /// The control connection switched sessions.
    SessionChanged {
        /// New tmux session handle.
        session: String,
    },
    /// The session list may have changed.
    SessionsChanged,
    /// The session was renamed.
    SessionRenamed {
        /// tmux session handle.
        session: String,
        /// New session name.
        name: String,
    },
    /// The session was removed.
    SessionClosed {
        /// tmux session handle.
        session: String,
    },
    /// The active pane of a window changed.
    WindowPaneChanged {
        /// tmux window handle.
        window: String,
        /// tmux pane handle.
        pane: String,
    },
    /// A window that is not linked to any session was added.
    UnlinkedWindowAdd {
        /// tmux window handle.
        handle: String,
    },
    /// A window that is not linked to any session was closed.
    UnlinkedWindowClose {
        /// tmux window handle.
        handle: String,
    },
    /// A pane entered or left a mode.
    PaneModeChanged {
        /// tmux pane handle.
        pane: String,
    },
    /// A pane's program exited cleanly.
    PaneExited {
        /// tmux pane handle.
        pane: String,
    },
    /// A pane's program died abnormally.
    PaneDied {
        /// tmux pane handle.
        pane: String,
    },
    /// A paste buffer changed.
    PasteBufferChanged {
        /// Buffer name.
        name: String,
    },
    /// A client detached from the server.
    ClientDetached {
        /// Client identifier.
        client: String,
    },
    /// A client switched sessions.
    ClientSessionChanged {
        /// Client identifier.
        client: String,
        /// tmux session handle.
        session: String,
        /// Session name.
        name: String,
    },
    /// The server rejected a config option on reload.
    ConfigError {
        /// Error message.
        message: String,
    },
    /// The server paused output for a pane (backpressure).
    Pause {
        /// tmux pane handle.
        pane: String,
    },
    /// The server resumed output for a pane.
    Continue {
        /// tmux pane handle.
        pane: String,
    },
    /// `%popup-open` (tmux 3.4+). Raw payload carried verbatim.
    PopupOpen {
        /// Raw line remainder.
        line: String,
    },
    /// `%popup-output` (tmux 3.4+). Raw payload carried verbatim.
    PopupOutput {
        /// Raw line remainder.
        line: String,
    },
    /// `%popup-close` (tmux 3.4+). Raw payload carried verbatim.
    PopupClose {
        /// Raw line remainder.
        line: String,
    },
    /// The tmux control connection exited.
    Exit {
        /// Exit reason supplied by tmux.
        reason: String,
    },
}

/// A cross-chunk-preserving parser: feed bytes, receive complete records.
#[derive(Debug, Default)]
pub struct ProtocolParser {
    carry: Vec<u8>,
    scanned: usize,
}

impl ProtocolParser {
    /// Create a parser with no retained state.
    pub fn new() -> Self {
        Self::default()
    }

    /// Feed a chunk of control-mode bytes and return every complete record
    /// it newly produced. Incomplete trailing lines are retained internally.
    pub fn feed(&mut self, chunk: &[u8]) -> Vec<Parsed> {
        self.carry.extend_from_slice(chunk);
        let mut out = Vec::new();
        while let Some(line) = self.take_line() {
            out.push(parse_line(&line));
        }
        out
    }

    fn take_line(&mut self) -> Option<Vec<u8>> {
        let rel = self.carry[self.scanned..].iter().position(|&b| b == b'\n');
        let Some(rel) = rel else {
            self.scanned = self.carry.len();
            return None;
        };
        let end = self.scanned + rel;
        let mut line: Vec<u8> = self.carry.drain(..=end).collect();
        line.pop();
        if line.last() == Some(&b'\r') {
            line.pop();
        }
        self.scanned = 0;
        Some(line)
    }
}

/// Decode tmux control-mode string escapes into raw bytes.
pub fn unescape(data: &str) -> Vec<u8> {
    unescape_bytes(data.as_bytes())
}

fn unescape_bytes(raw: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(raw.len());
    let mut i = 0;
    while i < raw.len() {
        let byte = raw[i];
        if byte != b'\\' {
            out.push(byte);
            i += 1;
            continue;
        }
        match raw.get(i + 1).copied() {
            None => {
                out.push(b'\\');
                i += 1;
            }
            Some(b'\\') => {
                out.push(b'\\');
                i += 2;
            }
            Some(first @ b'0'..=b'7') => {
                let mut value = u32::from(first - b'0');
                let mut digits = 1;
                while digits < 3 {
                    match raw.get(i + 1 + digits).copied() {
                        Some(next @ b'0'..=b'7') => {
                            value = value * 8 + u32::from(next - b'0');
                            digits += 1;
                        }
                        _ => break,
                    }
                }
                out.push((value & 0xff) as u8);
                i += 1 + digits;
            }
            Some(other) => {
                out.push(b'\\');
                out.push(other);
                i += 2;
            }
        }
    }
    out
}

fn lossy(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

fn parse_line(line: &[u8]) -> Parsed {
    if line.first() != Some(&b'%') {
        return Parsed::Unknown(lossy(line));
    }

    if let Some(rest) = line.strip_prefix(b"%output ") {
        return match split_token(rest) {
            Some((pane, payload)) => Parsed::Output {
                pane: lossy(pane),
                bytes: unescape_bytes(payload),
            },
            None => Parsed::Unknown(lossy(line)),
        };
    }

    if let Some(rest) = line.strip_prefix(b"%extended-output ") {
        return parse_extended_output(rest).unwrap_or_else(|| Parsed::Unknown(lossy(line)));
    }

    let text = lossy(line);
    let tokens: Vec<&str> = text.split_whitespace().collect();
    let head = tokens.first().copied().unwrap_or("");

    match head {
        "%begin" => match num_field(&tokens, 2) {
            Some(num) => Parsed::Begin {
                num,
                flags: flags_field(&tokens, 3).unwrap_or(0),
            },
            None => Parsed::Unknown(text),
        },
        "%end" => match num_field(&tokens, 2) {
            Some(num) => Parsed::End { num },
            None => Parsed::Unknown(text),
        },
        "%error" => match num_field(&tokens, 2) {
            Some(num) => Parsed::Error {
                num,
                message: rest_after(&text, 4).to_string(),
            },
            None => Parsed::Unknown(text),
        },
        "%window-add" => one_arg(&text, &tokens, |handle| TmuxEvent::WindowAdd { handle }),
        "%window-close" => one_arg(&text, &tokens, |handle| TmuxEvent::WindowClose { handle }),
        "%window-renamed" => match tokens.get(1) {
            Some(handle) => Parsed::Event(TmuxEvent::WindowRenamed {
                handle: (*handle).to_string(),
                name: tokens[2..].join(" "),
            }),
            None => Parsed::Unknown(text),
        },
        "%unlinked-window-add" => one_arg(&text, &tokens, |handle| TmuxEvent::UnlinkedWindowAdd {
            handle,
        }),
        "%unlinked-window-close" => one_arg(&text, &tokens, |handle| {
            TmuxEvent::UnlinkedWindowClose { handle }
        }),
        "%session-changed" => one_arg(&text, &tokens, |session| TmuxEvent::SessionChanged {
            session,
        }),
        "%session-renamed" => match tokens.get(1) {
            Some(session) => Parsed::Event(TmuxEvent::SessionRenamed {
                session: (*session).to_string(),
                name: tokens[2..].join(" "),
            }),
            None => Parsed::Unknown(text),
        },
        "%session-closed" => match tokens.get(1) {
            Some(session) => Parsed::Event(TmuxEvent::SessionClosed {
                session: (*session).to_string(),
            }),
            None => Parsed::Unknown(text),
        },
        "%layout-change" => match tokens.get(1) {
            Some(handle) => Parsed::Event(TmuxEvent::LayoutChange {
                handle: (*handle).to_string(),
                layout: tokens
                    .get(3)
                    .or_else(|| tokens.get(2))
                    .copied()
                    .unwrap_or_default()
                    .to_string(),
            }),
            None => Parsed::Unknown(text),
        },
        "%session-window-changed" => match (tokens.get(1), tokens.get(2)) {
            (Some(session), Some(window)) => Parsed::Event(TmuxEvent::SessionWindowChanged {
                session: (*session).to_string(),
                window: (*window).to_string(),
            }),
            _ => Parsed::Unknown(text),
        },
        "%window-pane-changed" => match (tokens.get(1), tokens.get(2)) {
            (Some(window), Some(pane)) => Parsed::Event(TmuxEvent::WindowPaneChanged {
                window: (*window).to_string(),
                pane: (*pane).to_string(),
            }),
            _ => Parsed::Unknown(text),
        },
        "%sessions-changed" => Parsed::Event(TmuxEvent::SessionsChanged),
        "%pane-mode-changed" => one_arg(&text, &tokens, |pane| TmuxEvent::PaneModeChanged { pane }),
        "%pane-exited" => one_arg(&text, &tokens, |pane| TmuxEvent::PaneExited { pane }),
        "%pane-died" => one_arg(&text, &tokens, |pane| TmuxEvent::PaneDied { pane }),
        "%paste-buffer-changed" => one_arg(&text, &tokens, |name| TmuxEvent::PasteBufferChanged {
            name,
        }),
        "%client-detached" => one_arg(&text, &tokens, |client| TmuxEvent::ClientDetached {
            client,
        }),
        "%client-session-changed" => match (tokens.get(1), tokens.get(2)) {
            (Some(client), Some(session)) => Parsed::Event(TmuxEvent::ClientSessionChanged {
                client: (*client).to_string(),
                session: (*session).to_string(),
                name: tokens[3..].join(" "),
            }),
            _ => Parsed::Unknown(text),
        },
        "%config-error" => Parsed::Event(TmuxEvent::ConfigError {
            message: rest_after(&text, 1).to_string(),
        }),
        "%pause" => one_arg(&text, &tokens, |pane| TmuxEvent::Pause { pane }),
        "%continue" => one_arg(&text, &tokens, |pane| TmuxEvent::Continue { pane }),
        "%popup-open" => Parsed::Event(TmuxEvent::PopupOpen {
            line: rest_after(&text, 1).to_string(),
        }),
        "%popup-output" => Parsed::Event(TmuxEvent::PopupOutput {
            line: rest_after(&text, 1).to_string(),
        }),
        "%popup-close" => Parsed::Event(TmuxEvent::PopupClose {
            line: rest_after(&text, 1).to_string(),
        }),
        "%exit" => Parsed::Event(TmuxEvent::Exit {
            reason: rest_after(&text, 1).to_string(),
        }),
        _ => Parsed::Unknown(text),
    }
}

fn parse_extended_output(rest: &[u8]) -> Option<Parsed> {
    let text = lossy(rest);
    let tokens: Vec<&str> = text.split_whitespace().collect();
    if tokens.len() < 4 {
        return None;
    }
    let pane = tokens[0].to_string();
    let age_ms = tokens[1].parse::<u64>().ok()?;
    let tail = tokens[2..].join(" ");
    let data_str = tail.rsplit_once(':')?.1;
    Some(Parsed::ExtendedOutput {
        pane,
        age_ms,
        bytes: unescape_bytes(data_str.trim_start().as_bytes()),
    })
}

fn one_arg(text: &str, tokens: &[&str], make: impl FnOnce(String) -> TmuxEvent) -> Parsed {
    match tokens.get(1) {
        Some(value) => Parsed::Event(make((*value).to_string())),
        None => Parsed::Unknown(text.to_string()),
    }
}

fn num_field(tokens: &[&str], index: usize) -> Option<u64> {
    tokens.get(index)?.parse().ok()
}

fn flags_field(tokens: &[&str], index: usize) -> Option<u32> {
    tokens.get(index)?.parse().ok()
}

fn split_token(bytes: &[u8]) -> Option<(&[u8], &[u8])> {
    let at = bytes.iter().position(|&b| b == b' ' || b == b'\t')?;
    let rest = &bytes[at..];
    let skip = rest
        .iter()
        .position(|&b| b != b' ' && b != b'\t')
        .unwrap_or(rest.len());
    Some((&bytes[..at], &rest[skip..]))
}

fn rest_after(text: &str, words: usize) -> &str {
    let bytes = text.as_bytes();
    let mut i = 0;
    for _ in 0..words {
        while i < bytes.len() && bytes[i] != b' ' && bytes[i] != b'\t' {
            i += 1;
        }
        while i < bytes.len() && (bytes[i] == b' ' || bytes[i] == b'\t') {
            i += 1;
        }
    }
    &text[i..]
}

#[cfg(test)]
mod tests {
    use super::*;

    const TRANSCRIPT: &str = concat!(
        "%begin 1792566490 270 0\r\n",
        "%end 1792566490 270 0\r\n",
        "%session-changed $0 probe\r\n",
        "%output %0 TICK\\015\\012\r\n",
        "%extended-output %0 1234 0,1,2 : hi\\041\r\n",
        "%window-add @1\r\n",
        "%session-window-changed $0 @1\r\n",
        "%window-pane-changed @1 %2\r\n",
        "%pane-exited %2\r\n",
        "%pause %0\r\n",
        "%continue %0\r\n",
        "%unlinked-window-close @1\r\n",
        "%sessions-changed\r\n",
        "%exit\r\n",
    );

    fn parse_all(input: &str) -> Vec<Parsed> {
        ProtocolParser::new().feed(input.as_bytes())
    }

    #[test]
    fn transcript_parses() {
        let parsed = parse_all(TRANSCRIPT);
        assert!(parsed.contains(&Parsed::Begin { num: 270, flags: 0 }));
        assert!(parsed.contains(&Parsed::End { num: 270 }));
        assert!(
            parsed.contains(&Parsed::Event(TmuxEvent::WindowPaneChanged {
                window: "@1".into(),
                pane: "%2".into(),
            }))
        );
        assert!(parsed.contains(&Parsed::Event(TmuxEvent::PaneExited { pane: "%2".into() })));
        assert!(parsed.contains(&Parsed::Event(TmuxEvent::Pause { pane: "%0".into() })));
        assert!(
            parsed.contains(&Parsed::Event(TmuxEvent::UnlinkedWindowClose {
                handle: "@1".into(),
            }))
        );
        assert!(parsed.contains(&Parsed::ExtendedOutput {
            pane: "%0".into(),
            age_ms: 1234,
            bytes: b"hi!".to_vec(),
        }));
        assert!(!parsed.iter().any(|p| matches!(p, Parsed::Unknown(_))));
    }

    #[test]
    fn split_feeds_are_identical() {
        let whole = parse_all(TRANSCRIPT);
        let mut parser = ProtocolParser::new();
        let mut bytewise = Vec::new();
        for byte in TRANSCRIPT.as_bytes() {
            bytewise.extend(parser.feed(&[*byte]));
        }
        assert_eq!(bytewise, whole);
    }

    #[test]
    fn utf8_survives_escaping() {
        let parsed = parse_all("%output %2 h\u{e9}llo\u{2192}\x1b[1m\r\n");
        let expected = "h\u{e9}llo\u{2192}\x1b[1m".as_bytes().to_vec();
        match &parsed[0] {
            Parsed::Output { pane, bytes } => {
                assert_eq!(pane, "%2");
                assert_eq!(bytes, &expected);
            }
            other => panic!("expected Output, got {other:?}"),
        }
        assert_eq!(unescape(r"a\\b"), b"a\\b");
        assert_eq!(unescape("tail\\"), b"tail\\");
    }
}
