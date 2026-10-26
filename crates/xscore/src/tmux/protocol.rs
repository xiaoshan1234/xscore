// tmux/protocol.rs — the `tmux -CC` control-mode wire format, parsed without any IO.
//
// Everything here is pure: `ProtocolParser::feed` takes bytes and returns
// `Parsed` values, so each rule in ADR 0010 §D2 and arch.md §7 can be tested
// against one captured tmux 3.5a transcript instead of a live server.
//
// The rules that shape this file, all measured (ADR 0010 §D2 table):
//   * a reply is bracketed by `%begin <ts> <num> <flags>` … `%end`/`%error`,
//     and is correlated by `<num>`, never by arrival order;
//   * events may arrive *inside* a block, so routing is by line prefix, never
//     by position;
//   * `%output %N <escaped>` is the only notification carrying bytes: one line
//     is one complete record because tmux escapes every newline (`\012`);
//   * a read can still split a line, so terminated lines are the unit of work.

/// One protocol line, classified.
#[derive(Debug, Clone, PartialEq)]
pub enum Parsed {
    Begin {
        num: u64,
        flags: u32,
    },
    End {
        num: u64,
    },
    Error {
        num: u64,
        message: String,
    },
    /// Pane bytes. `pane` is the tmux handle (`"%3"`), which is the link key a
    /// pane addresses itself with (ADR 0010 §D3).
    Output {
        pane: String,
        bytes: Vec<u8>,
    },
    Event(TmuxEvent),
    /// Neither framing nor pane bytes nor a notification this build knows:
    /// either the text output of the block that is open, or an unknown `%`
    /// notification. `control::reader_loop` decides between the two by asking
    /// whether a block is open and whether the text starts with `%`.
    Unknown(String),
}

/// A structural notification — "events, not polling" (arch.md §7).
#[derive(Debug, Clone, PartialEq)]
pub enum TmuxEvent {
    WindowAdd {
        handle: String,
    },
    WindowClose {
        handle: String,
    },
    WindowRenamed {
        handle: String,
    },
    /// `layout` is the *new* layout string (the 3rd field); the old one and the
    /// trailing flags are dropped — geometry is read from the new value only.
    LayoutChange {
        handle: String,
        layout: String,
    },
    SessionWindowChanged {
        session: String,
        window: String,
    },
    SessionChanged {
        session: String,
    },
    SessionsChanged,
    Exit {
        reason: String,
    },
}

/// Carry-over parser: feed it whatever a read returned, get completed records.
#[derive(Debug, Default)]
pub struct ProtocolParser {
    /// Bytes received but not yet terminated by `\n`.
    carry: Vec<u8>,
    /// How much of `carry` has already been searched for `\n`, so a long
    /// unterminated `%output` line is not rescanned from the start on every
    /// read (a screen redraw arrives as one very long line).
    scanned: usize,
}

impl ProtocolParser {
    pub fn new() -> Self {
        Self::default()
    }

    /// Consume one chunk and return every record it completed. Partial lines
    /// stay in the carry-over buffer until their `\n` arrives.
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
        line.pop(); // the `\n` itself
        if line.last() == Some(&b'\r') {
            // tmux writes CRLF; the `\r` is framing, not payload.
            line.pop();
        }
        self.scanned = 0;
        Some(line)
    }
}

/// Unescape `%output`'s payload: `\ooo` is one octal byte, `\\` a literal
/// backslash, anything else is kept literally (both bytes — tmux never emits
/// those, and dropping the backslash would corrupt data we did not understand).
pub fn unescape(data: &str) -> Vec<u8> {
    unescape_bytes(data.as_bytes())
}

/// The real implementation. It takes bytes, not a `&str`, because UTF-8 passes
/// through `%output` unescaped: decoding first and unescaping per `char` would
/// either mangle or reject a multi-byte sequence (ADR 0010 §D2).
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
        // No prefix: a command's output line, or an empty keep-alive line.
        return Parsed::Unknown(lossy(line));
    }

    // `%output` is parsed on the raw slice: the payload is bytes, and the split
    // between the pane id and the data must not go through a `String`.
    if let Some(rest) = line.strip_prefix(b"%output ") {
        return match split_token(rest) {
            Some((pane, payload)) => Parsed::Output {
                pane: lossy(pane),
                bytes: unescape_bytes(payload),
            },
            None => Parsed::Unknown(lossy(line)),
        };
    }

    let text = lossy(line);
    let tokens: Vec<&str> = text.split_whitespace().collect();
    let head = tokens.first().copied().unwrap_or("");

    match head {
        // `%begin <ts> <num> <flags>`; the timestamp is ignored — only `<num>`
        // identifies the reply (ADR 0010 §D2).
        "%begin" => match (num_field(&tokens, 2), flags_field(&tokens, 3)) {
            (Some(num), Some(flags)) => Parsed::Begin { num, flags },
            _ => Parsed::Unknown(text),
        },
        "%end" => match num_field(&tokens, 2) {
            Some(num) => Parsed::End { num },
            None => Parsed::Unknown(text),
        },
        "%error" => match num_field(&tokens, 2) {
            Some(num) => Parsed::Error {
                num,
                // The text arrives after the flags; a bare `%error` is legal.
                message: rest_after(&text, 4).to_string(),
            },
            None => Parsed::Unknown(text),
        },
        "%window-add" => one_arg(&text, &tokens, |handle| TmuxEvent::WindowAdd { handle }),
        // 3.5a emits `%unlinked-window-close`; older releases emitted
        // `%window-close` for the same thing. Both mean "this window is gone".
        "%window-close" | "%unlinked-window-close" => {
            one_arg(&text, &tokens, |handle| TmuxEvent::WindowClose { handle })
        }
        "%window-renamed" => one_arg(&text, &tokens, |handle| TmuxEvent::WindowRenamed { handle }),
        // `%session-changed $0 name` — the session's *handle* is identity; the
        // trailing name is display-only and can change under us (ADR 0008).
        "%session-changed" => one_arg(&text, &tokens, |session| TmuxEvent::SessionChanged {
            session,
        }),
        "%layout-change" => match tokens.get(1) {
            // `@N <old> <new> <flags>`; 2-field forms exist, so fall back.
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
        "%sessions-changed" => Parsed::Event(TmuxEvent::SessionsChanged),
        "%exit" => Parsed::Event(TmuxEvent::Exit {
            reason: rest_after(&text, 1).to_string(),
        }),
        // Unknown notification: never fatal — tmux renames these across
        // versions, and a build that treats them as errors drops the whole
        // session on the next release (arch.md §7).
        _ => Parsed::Unknown(text),
    }
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

/// Split at the first space/tab: `(token, remainder with leading blanks removed)`.
fn split_token(bytes: &[u8]) -> Option<(&[u8], &[u8])> {
    let at = bytes.iter().position(|&b| b == b' ' || b == b'\t')?;
    let rest = &bytes[at..];
    let skip = rest
        .iter()
        .position(|&b| b != b' ' && b != b'\t')
        .unwrap_or(rest.len());
    Some((&bytes[..at], &rest[skip..]))
}

/// Everything after the first `words` whitespace-delimited words. Slices are
/// only ever cut at ASCII separators, so this is UTF-8 safe.
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

    /// Captured from tmux 3.5a, verbatim (CRLF line ends, escapes intact).
    const TRANSCRIPT: &str = concat!(
        "%begin 1792566490 270 0\r\n",
        "%end 1792566490 270 0\r\n",
        "%session-changed $0 probe\r\n",
        "%output %0 TICK\\015\\012\r\n",
        "%output %1 \\033[?2004hloner@WinLite \\033[0;36mxscore\\033[0m $ \r\n",
        "%window-add @1\r\n",
        "%session-window-changed $0 @1\r\n",
        "%layout-change @1 419a,80x24,0,0[80x12,0,0,1,80x11,0,13,2] \
         419a,80x24,0,0[80x12,0,0,1,80x11,0,13,2] *\r\n",
        "%unlinked-window-close @1\r\n",
        "%sessions-changed\r\n",
        "%exit\r\n",
    );

    fn parse_all(input: &str) -> Vec<Parsed> {
        ProtocolParser::new().feed(input.as_bytes())
    }

    fn outputs(parsed: &[Parsed]) -> Vec<(String, Vec<u8>)> {
        parsed
            .iter()
            .filter_map(|p| match p {
                Parsed::Output { pane, bytes } => Some((pane.clone(), bytes.clone())),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn transcript_parses() {
        let parsed = parse_all(TRANSCRIPT);

        // Framing is present and correlated by num.
        assert!(parsed.contains(&Parsed::Begin { num: 270, flags: 0 }));
        assert!(parsed.contains(&Parsed::End { num: 270 }));

        let bytes = outputs(&parsed);
        assert_eq!(bytes.len(), 2, "two %output records expected: {parsed:?}");
        assert_eq!(bytes[0].0, "%0");
        assert_eq!(bytes[0].1, b"TICK\r\n");
        assert_eq!(bytes[1].0, "%1");
        assert!(
            bytes[1].1.starts_with(b"\x1b[?2004h"),
            "escape not decoded: {:?}",
            bytes[1].1
        );
        assert!(
            bytes[1].1.ends_with(b"$ "),
            "payload truncated: {:?}",
            bytes[1].1
        );

        // Events.
        assert!(parsed.contains(&Parsed::Event(TmuxEvent::WindowAdd {
            handle: "@1".into()
        })));
        assert!(parsed.contains(&Parsed::Event(TmuxEvent::SessionChanged {
            session: "$0".into()
        })));
        assert!(
            parsed.contains(&Parsed::Event(TmuxEvent::SessionWindowChanged {
                session: "$0".into(),
                window: "@1".into(),
            }))
        );
        assert!(
            parsed.iter().any(|p| matches!(
                p,
                Parsed::Event(TmuxEvent::LayoutChange { handle, layout })
                    if handle == "@1"
                        && layout
                            == "419a,80x24,0,0[80x12,0,0,1,80x11,0,13,2]"
            )),
            "layout-change did not carry the new layout: {parsed:?}"
        );
        // `%unlinked-window-close` is today's name for `%window-close`.
        assert!(parsed.contains(&Parsed::Event(TmuxEvent::WindowClose {
            handle: "@1".into()
        })));
        assert!(parsed.contains(&Parsed::Event(TmuxEvent::SessionsChanged)));
        assert!(parsed.contains(&Parsed::Event(TmuxEvent::Exit {
            reason: String::new()
        })));
        // Every line in the transcript is understood — no fallthrough.
        assert!(
            !parsed.iter().any(|p| matches!(p, Parsed::Unknown(_))),
            "unparsed line in the transcript: {parsed:?}"
        );
    }

    #[test]
    fn split_feeds_are_identical() {
        let whole = parse_all(TRANSCRIPT);

        // Every two-chunk split point: no line boundary carries hidden state.
        for at in 1..TRANSCRIPT.len() {
            let mut parser = ProtocolParser::new();
            let mut parsed = parser.feed(&TRANSCRIPT.as_bytes()[..at]);
            parsed.extend(parser.feed(&TRANSCRIPT.as_bytes()[at..]));
            assert_eq!(parsed, whole, "split at byte {at} changed the parse");
        }

        // And the pathological case: one byte per read.
        let mut parser = ProtocolParser::new();
        let mut bytewise = Vec::new();
        for byte in TRANSCRIPT.as_bytes() {
            bytewise.extend(parser.feed(&[*byte]));
        }
        assert_eq!(bytewise, whole, "byte-at-a-time feed changed the parse");
    }

    #[test]
    fn utf8_survives_escaping() {
        let parsed = parse_all("%output %2 héllo→\\033[1m\r\n");
        let expected = "héllo→\x1b[1m".as_bytes().to_vec();
        assert_eq!(parsed.len(), 1);
        match &parsed[0] {
            Parsed::Output { pane, bytes } => {
                assert_eq!(pane, "%2");
                assert_eq!(bytes, &expected);
            }
            other => panic!("expected Output, got {other:?}"),
        }

        // `\\` is one backslash; a lone trailing `\` does not vanish.
        assert_eq!(unescape(r"a\\b"), b"a\\b");
        assert_eq!(unescape("tail\\"), b"tail\\");
        // Unknown sequences are kept literally rather than guessed at.
        assert_eq!(unescape(r"\q"), b"\\q");
    }

    #[test]
    fn unknown_notifications_ignored() {
        let mut parser = ProtocolParser::new();
        let first = parser.feed(b"%something-new 1 2\r\n");
        assert_eq!(first.len(), 1);
        assert!(matches!(first[0], Parsed::Unknown(_)));
        assert!(
            !first.iter().any(|p| matches!(p, Parsed::Event(_))),
            "an unknown notification must not produce an event"
        );

        // The stream keeps working afterwards.
        let second = parser.feed(b"%window-add @3\r\n");
        assert_eq!(
            second,
            vec![Parsed::Event(TmuxEvent::WindowAdd {
                handle: "@3".into()
            })]
        );

        // An unterminated line is buffered, not parsed.
        let mut parser = ProtocolParser::new();
        assert!(parser.feed(b"%window-a").is_empty());
        assert_eq!(
            parser.feed(b"dd @4\n"),
            vec![Parsed::Event(TmuxEvent::WindowAdd {
                handle: "@4".into()
            })]
        );
    }

    #[test]
    fn event_inside_a_block_is_still_an_event() {
        // arch.md §7: routing is by prefix, not by position.
        let parsed = parse_all(concat!(
            "%begin 1 7 0\r\n",
            "line one\r\n",
            "%window-add @9\r\n",
            "line two\r\n",
            "%end 1 7 0\r\n",
        ));
        assert_eq!(
            parsed,
            vec![
                Parsed::Begin { num: 7, flags: 0 },
                Parsed::Unknown("line one".into()),
                Parsed::Event(TmuxEvent::WindowAdd {
                    handle: "@9".into()
                }),
                Parsed::Unknown("line two".into()),
                Parsed::End { num: 7 },
            ]
        );
    }

    #[test]
    fn error_block_carries_its_message() {
        let parsed = parse_all("%error 1792566490 12 0 can't find session: nope\r\n");
        assert_eq!(
            parsed,
            vec![Parsed::Error {
                num: 12,
                message: "can't find session: nope".into()
            }]
        );
        // A bare `%error` is legal and must not be dropped as unknown.
        assert_eq!(
            parse_all("%error 1 5 0\n"),
            vec![Parsed::Error {
                num: 5,
                message: String::new()
            }]
        );
    }

    #[test]
    fn exit_carries_reason() {
        assert_eq!(
            parse_all("%exit detached\n"),
            vec![Parsed::Event(TmuxEvent::Exit {
                reason: "detached".into()
            })]
        );
    }
}
