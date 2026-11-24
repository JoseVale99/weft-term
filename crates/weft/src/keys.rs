//! Decoding of raw terminal input into semantic keys.
//!
//! The front-end owns no terminal knowledge beyond this table: the shell sends
//! bytes, the decoder produces keys, and the editor applies operations. Escape
//! sequences may arrive split across reads, so an incomplete sequence is kept
//! and retried with the next chunk instead of being reported as an error.

/// A key press the application understands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    /// A printable character.
    Char(char),
    /// Submit the line.
    Enter,
    /// Insert a line break instead of submitting.
    AltEnter,
    /// Request completion.
    Tab,
    /// Delete the character before the cursor.
    Backspace,
    /// Delete the character under the cursor.
    Delete,
    /// Escape pressed alone.
    Esc,
    /// Cursor left.
    Left,
    /// Cursor right.
    Right,
    /// Cursor up.
    Up,
    /// Cursor down.
    Down,
    /// Previous word.
    WordLeft,
    /// Next word.
    WordRight,
    /// Start of line.
    Home,
    /// End of line.
    End,
    /// Cancel the current line.
    CtrlC,
    /// End of input; exits when the line is empty.
    CtrlD,
    /// Delete to end of line.
    CtrlK,
    /// Clear the screen.
    CtrlL,
    /// Delete to start of line.
    CtrlU,
    /// Delete the previous word.
    CtrlW,
    /// Yank the last killed text.
    CtrlY,
    /// Previous history entry.
    CtrlP,
    /// Next history entry.
    CtrlN,
}

/// Incremental key decoder.
#[derive(Debug, Default)]
pub struct Decoder {
    buffer: Vec<u8>,
}

impl Decoder {
    /// Creates an empty decoder.
    pub fn new() -> Self {
        Self::default()
    }

    /// Appends bytes read from the terminal.
    pub fn push(&mut self, bytes: &[u8]) {
        self.buffer.extend_from_slice(bytes);
    }

    /// Returns the next complete key, or `None` when more bytes are needed.
    pub fn next(&mut self) -> Option<Key> {
        loop {
            let first = *self.buffer.first()?;
            let key = match first {
                0x1b => match self.escape()? {
                    Some(key) => key,
                    // An incomplete sequence: keep the bytes for the next read.
                    None => return None,
                },
                0x7f | 0x08 => Key::Backspace,
                b'\r' | b'\n' => Key::Enter,
                b'\t' => Key::Tab,
                0x03 => Key::CtrlC,
                0x04 => Key::CtrlD,
                0x0b => Key::CtrlK,
                0x0c => Key::CtrlL,
                0x15 => Key::CtrlU,
                0x17 => Key::CtrlW,
                0x19 => Key::CtrlY,
                0x10 => Key::CtrlP,
                0x0e => Key::CtrlN,
                byte if byte < 0x20 => {
                    // Unknown control character: drop it and keep decoding.
                    self.buffer.remove(0);
                    continue;
                }
                _ => match self.utf8() {
                    // Incomplete sequence: wait for the next read.
                    None => return None,
                    // Invalid byte: already dropped, keep decoding.
                    Some(None) => continue,
                    Some(Some(key)) => key,
                },
            };
            return Some(key);
        }
    }

    fn escape(&mut self) -> Option<Option<Key>> {
        let Some(second) = self.buffer.get(1).copied() else {
            return Some(None);
        };
        match second {
            b'[' => {
                let Some(end) = self
                    .buffer
                    .iter()
                    .enumerate()
                    .skip(2)
                    .find(|(_, byte)| (0x40..=0x7e).contains(*byte))
                    .map(|(index, _)| index)
                else {
                    return Some(None);
                };
                let params = String::from_utf8_lossy(&self.buffer[2..end]).into_owned();
                let final_byte = self.buffer[end];
                self.buffer.drain(..=end);
                Some(Some(csi(&params, final_byte)))
            }
            b'O' => {
                let Some(third) = self.buffer.get(2).copied() else {
                    return Some(None);
                };
                self.buffer.drain(..3);
                Some(Some(ss3(third)))
            }
            b'\r' => {
                self.buffer.drain(..2);
                Some(Some(Key::AltEnter))
            }
            b'b' => {
                self.buffer.drain(..2);
                Some(Some(Key::WordLeft))
            }
            b'f' => {
                self.buffer.drain(..2);
                Some(Some(Key::WordRight))
            }
            _ => {
                self.buffer.drain(..2);
                Some(Some(Key::Esc))
            }
        }
    }

    fn utf8(&mut self) -> Option<Option<Key>> {
        match std::str::from_utf8(&self.buffer) {
            Ok(text) => {
                let c = text.chars().next()?;
                self.buffer.drain(..c.len_utf8());
                Some(Some(Key::Char(c)))
            }
            Err(error) if error.valid_up_to() > 0 => {
                let text = std::str::from_utf8(&self.buffer[..error.valid_up_to()]).ok()?;
                let c = text.chars().next()?;
                self.buffer.drain(..c.len_utf8());
                Some(Some(Key::Char(c)))
            }
            Err(error) if error.error_len().is_none() => Some(None),
            Err(_) => {
                // Invalid byte: drop it so the stream can resynchronize.
                self.buffer.remove(0);
                Some(None)
            }
        }
    }
}

fn csi(params: &str, final_byte: u8) -> Key {
    let modifier = params
        .rsplit_once(';')
        .map(|(_, modifier)| modifier)
        .unwrap_or("");
    match final_byte {
        b'A' => Key::Up,
        b'B' => Key::Down,
        b'C' if modifier.starts_with('5') || modifier.starts_with('3') => Key::WordRight,
        b'C' => Key::Right,
        b'D' if modifier.starts_with('5') || modifier.starts_with('3') => Key::WordLeft,
        b'D' => Key::Left,
        b'H' => Key::Home,
        b'F' => Key::End,
        b'~' => match params {
            "1" | "7" => Key::Home,
            "4" | "8" => Key::End,
            "3" => Key::Delete,
            _ => Key::Esc,
        },
        _ => Key::Esc,
    }
}

fn ss3(byte: u8) -> Key {
    match byte {
        b'A' => Key::Up,
        b'B' => Key::Down,
        b'C' => Key::Right,
        b'D' => Key::Left,
        b'H' => Key::Home,
        b'F' => Key::End,
        _ => Key::Esc,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decode(bytes: &[u8]) -> Vec<Key> {
        let mut decoder = Decoder::new();
        decoder.push(bytes);
        let mut keys = Vec::new();
        while let Some(key) = decoder.next() {
            keys.push(key);
        }
        keys
    }

    #[test]
    fn printable_ascii_is_decoded() {
        assert_eq!(decode(b"ls"), [Key::Char('l'), Key::Char('s')]);
    }

    #[test]
    fn control_keys_are_decoded() {
        assert_eq!(
            decode(b"\x03\x04\x0b\x0c\x15\x17\x19\x10\x0e\r\t\x7f"),
            [
                Key::CtrlC,
                Key::CtrlD,
                Key::CtrlK,
                Key::CtrlL,
                Key::CtrlU,
                Key::CtrlW,
                Key::CtrlY,
                Key::CtrlP,
                Key::CtrlN,
                Key::Enter,
                Key::Tab,
                Key::Backspace,
            ]
        );
    }

    #[test]
    fn arrows_are_decoded_in_both_forms() {
        assert_eq!(
            decode(b"\x1b[A\x1b[B\x1b[C\x1b[D\x1bOA\x1bOB"),
            [Key::Up, Key::Down, Key::Right, Key::Left, Key::Up, Key::Down]
        );
    }

    #[test]
    fn modified_arrows_move_by_word() {
        assert_eq!(
            decode(b"\x1b[1;5C\x1b[1;3D"),
            [Key::WordRight, Key::WordLeft]
        );
    }

    #[test]
    fn special_keys_are_decoded() {
        assert_eq!(
            decode(b"\x1b[H\x1b[4~\x1b[3~\x1b[F"),
            [Key::Home, Key::End, Key::Delete, Key::End]
        );
    }

    #[test]
    fn alt_enter_inserts_a_line_break() {
        assert_eq!(decode(b"\x1b\r"), [Key::AltEnter]);
    }

    #[test]
    fn a_sequence_split_across_reads_waits_for_the_rest() {
        let mut decoder = Decoder::new();
        decoder.push(b"\x1b[");
        assert_eq!(decoder.next(), None);
        decoder.push(b"1;5C");
        assert_eq!(decoder.next(), Some(Key::WordRight));
    }

    #[test]
    fn a_split_utf8_character_is_decoded_once() {
        let bytes = "ñ".as_bytes();
        let mut decoder = Decoder::new();
        decoder.push(&bytes[..1]);
        assert_eq!(decoder.next(), None);
        decoder.push(&bytes[1..]);
        assert_eq!(decoder.next(), Some(Key::Char('ñ')));
    }

    #[test]
    fn invalid_bytes_are_dropped() {
        assert_eq!(decode(&[0xff, b'a']), [Key::Char('a')]);
    }
}
