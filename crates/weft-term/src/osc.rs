//! Incremental parser for semantic-prompt escapes (OSC 133 / OSC 633) and cwd
//! reporting (OSC 7).
//!
//! These sequences are what turn a flat byte stream into Warp-style *blocks*:
//! the shell tells us where the prompt starts and ends, when the command starts
//! running, and with which exit code it finished. This module is pure parsing:
//! it owns no terminal state and no I/O, so it can be exercised with plain
//! `cargo test` and replayed against recordings.
//!
//! Supported:
//!
//! ```text
//! ESC ] 133 ; A            BEL   prompt start
//! ESC ] 133 ; B            BEL   prompt end / input start
//! ESC ] 133 ; C            BEL   command executed / output start
//! ESC ] 133 ; D ; <code>   BEL   command finished
//! ESC ] 633 ; A..D ; <code> BEL  VS Code shell-integration dialect
//! ESC ] 7 ; file://h/p     BEL   current working directory
//! ```
//!
//! Unknown OSC payloads, CSI sequences and DCS/APC/PM/SOS strings are skipped
//! without corrupting the state machine, because they may legally contain
//! payload bytes that look like our markers.

/// Escape (0x1b), the introducer of OSC/CSI/DCS.
const ESC: u8 = 0x1b;
/// Bell (0x07), accepted as an OSC terminator.
const BEL: u8 = 0x07;
/// C1 String Terminator (0x9c), accepted as an OSC terminator.
const C1_ST: u8 = 0x9c;
/// C1 OSC introducer (0x9d).
const C1_OSC: u8 = 0x9d;
/// Upper bound for one OSC payload. Real payloads are tens of bytes; anything
/// larger is hostile or broken and is dropped instead of growing the buffer.
const MAX_PAYLOAD: usize = 4096;

/// A semantic event extracted from the shell's byte stream.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    /// `OSC 133;A` — the shell is about to draw its prompt.
    PromptStart,
    /// `OSC 133;B` — the prompt is done, the user input starts.
    PromptEnd,
    /// `OSC 133;C` — the command has been executed, output follows.
    CommandStart,
    /// `OSC 133;D;<code>` — the command finished. `None` when the shell did not
    /// report an exit code.
    CommandFinished { exit_code: Option<i32> },
    /// `OSC 7;file://host/path` — the shell changed directory.
    Cwd(String),
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum State {
    #[default]
    Ground,
    Escape,
    Osc,
    OscEscape,
    Csi,
    Skipped,
    SkippedEscape,
}

/// Chunk-safe state machine. Feed it whatever `read()` returned; it may split
/// an escape sequence anywhere.
#[derive(Debug, Default)]
pub struct Parser {
    state: State,
    payload: Vec<u8>,
}

impl Parser {
    pub fn new() -> Self {
        Self::default()
    }

    /// Forgets any partially parsed sequence. Use it when the stream is
    /// restarted (new session, PTY respawn).
    pub fn reset(&mut self) {
        self.state = State::Ground;
        self.payload.clear();
    }

    /// Consumes bytes and appends every completed event to `out`.
    pub fn feed(&mut self, input: &[u8], out: &mut Vec<Event>) {
        let mut index = 0;
        while index < input.len() {
            let byte = input[index];
            index += 1;
            match self.state {
                State::Ground => match byte {
                    ESC => self.state = State::Escape,
                    C1_OSC => {
                        self.payload.clear();
                        self.state = State::Osc;
                    }
                    _ => {}
                },
                State::Escape => match byte {
                    b']' => {
                        self.payload.clear();
                        self.state = State::Osc;
                    }
                    b'[' => self.state = State::Csi,
                    // DCS, SOS, PM, APC: skip until ST.
                    b'P' | b'X' | b'^' | b'_' => self.state = State::Skipped,
                    ESC => self.state = State::Escape,
                    _ => self.state = State::Ground,
                },
                State::Osc => match byte {
                    BEL | C1_ST => {
                        self.dispatch(out);
                        self.state = State::Ground;
                    }
                    ESC => self.state = State::OscEscape,
                    _ => {
                        if self.payload.len() < MAX_PAYLOAD {
                            self.payload.push(byte);
                        }
                    }
                },
                State::OscEscape => {
                    if byte == b'\\' {
                        self.dispatch(out);
                        self.state = State::Ground;
                    } else {
                        // Not `ESC \`: the OSC was aborted. Drop it and treat
                        // this byte as the start of a new sequence.
                        self.payload.clear();
                        self.state = State::Escape;
                        // Re-read this byte as an escape continuation.
                        index -= 1;
                    }
                }
                State::Csi => {
                    // Final byte of a CSI sequence: 0x40..=0x7e.
                    if (0x40..=0x7e).contains(&byte) {
                        self.state = State::Ground;
                    }
                }
                State::Skipped => match byte {
                    ESC => self.state = State::SkippedEscape,
                    C1_ST => self.state = State::Ground,
                    _ => {}
                },
                State::SkippedEscape => {
                    if byte == b'\\' {
                        self.state = State::Ground;
                    } else {
                        self.state = State::Escape;
                        index -= 1;
                    }
                }
            }
        }
    }

    fn dispatch(&mut self, out: &mut Vec<Event>) {
        let payload = String::from_utf8_lossy(&self.payload);
        if let Some(event) = parse_payload(&payload) {
            out.push(event);
        }
    }
}

/// Parses one complete OSC payload (the bytes between `ESC ]` and the
/// terminator) into a semantic event. Returns `None` for unrelated OSC codes.
pub fn parse_payload(payload: &str) -> Option<Event> {
    let mut parts = payload.split(';');
    match parts.next()? {
        "133" | "633" => match parts.next().map(str::trim) {
            Some("A") => Some(Event::PromptStart),
            Some("B") => Some(Event::PromptEnd),
            Some("C") => Some(Event::CommandStart),
            Some("D") => Some(Event::CommandFinished {
                exit_code: parts.next().and_then(|code| code.trim().parse().ok()),
            }),
            _ => None,
        },
        "7" => parts.next().map(|uri| Event::Cwd(uri.to_owned())),
        _ => None,
    }
}

/// Converts an OSC 7 `file://host/path` URI (or a bare path) into a path
/// string, decoding `%XX` escapes.
pub fn cwd_from_uri(uri: &str) -> String {
    let rest = uri.strip_prefix("file://").unwrap_or(uri);
    // Drop the authority: everything after the first `/` is the path. For
    // `file:///home/x` the authority is empty and the path starts at index 0.
    let path = match rest.find('/') {
        Some(index) => &rest[index..],
        None => rest,
    };
    percent_decode(path)
}

fn percent_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' && index + 2 < bytes.len() {
            let hex = |byte: u8| match byte {
                b'0'..=b'9' => Some(byte - b'0'),
                b'a'..=b'f' => Some(byte - b'a' + 10),
                b'A'..=b'F' => Some(byte - b'A' + 10),
                _ => None,
            };
            if let (Some(high), Some(low)) = (hex(bytes[index + 1]), hex(bytes[index + 2])) {
                out.push(high * 16 + low);
                index += 3;
                continue;
            }
        }
        out.push(bytes[index]);
        index += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}
