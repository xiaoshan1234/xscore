//! `tmux -CC` 控制模式线路协议：纯解析，无 IO。

/// 一行协议，按前缀分类。
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
    /// 窗格字节。`pane` 是 tmux 句柄（`"%3"`）。
    Output {
        pane: String,
        bytes: Vec<u8>,
    },
    Event(TmuxEvent),
    Unknown(String),
}

/// 结构通知。
#[derive(Debug, Clone, PartialEq)]
pub enum TmuxEvent {
    WindowAdd { handle: String },
    WindowClose { handle: String },
    WindowRenamed { handle: String },
    LayoutChange { handle: String, layout: String },
    SessionWindowChanged { session: String, window: String },
    SessionChanged { session: String },
    SessionsChanged,
    Exit { reason: String },
}

/// 跨读保留的解析器：喂入字节，返回完整记录。
#[derive(Debug, Default)]
pub struct ProtocolParser {
    carry: Vec<u8>,
    scanned: usize,
}

impl ProtocolParser {
    pub fn new() -> Self {
        Self::default()
    }

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

    let text = lossy(line);
    let tokens: Vec<&str> = text.split_whitespace().collect();
    let head = tokens.first().copied().unwrap_or("");

    match head {
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
                message: rest_after(&text, 4).to_string(),
            },
            None => Parsed::Unknown(text),
        },
        "%window-add" => one_arg(&text, &tokens, |handle| TmuxEvent::WindowAdd { handle }),
        "%window-close" | "%unlinked-window-close" => {
            one_arg(&text, &tokens, |handle| TmuxEvent::WindowClose { handle })
        }
        "%window-renamed" => one_arg(&text, &tokens, |handle| TmuxEvent::WindowRenamed { handle }),
        "%session-changed" => one_arg(&text, &tokens, |session| TmuxEvent::SessionChanged {
            session,
        }),
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
        "%sessions-changed" => Parsed::Event(TmuxEvent::SessionsChanged),
        "%exit" => Parsed::Event(TmuxEvent::Exit {
            reason: rest_after(&text, 1).to_string(),
        }),
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

    #[test]
    fn transcript_parses() {
        let parsed = parse_all(TRANSCRIPT);
        assert!(parsed.contains(&Parsed::Begin { num: 270, flags: 0 }));
        assert!(parsed.contains(&Parsed::End { num: 270 }));

        let outputs: Vec<(String, Vec<u8>)> = parsed
            .iter()
            .filter_map(|p| match p {
                Parsed::Output { pane, bytes } => Some((pane.clone(), bytes.clone())),
                _ => None,
            })
            .collect();
        assert_eq!(outputs.len(), 2);
        assert_eq!(outputs[0].0, "%0");
        assert_eq!(outputs[0].1, b"TICK\r\n");
        assert!(outputs[1].1.starts_with(b"\x1b[?2004h"));

        assert!(parsed.contains(&Parsed::Event(TmuxEvent::WindowAdd {
            handle: "@1".into()
        })));
        assert!(parsed.contains(&Parsed::Event(TmuxEvent::WindowClose {
            handle: "@1".into()
        })));
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
        let parsed = parse_all("%output %2 héllo→\\033[1m\r\n");
        let expected = "héllo→\x1b[1m".as_bytes().to_vec();
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
