//! Escape-sequence interpreter: bytes in, screen mutations and semantic events
//! out.
//!
//! The interpreter is an incremental state machine, so it can be fed whatever
//! `read()` returned, including a sequence split across two reads. What it does
//! not understand it skips, which is the only safe behavior for a terminal
//! attached to arbitrary programs.

use crate::blocks::Blocks;
use crate::grid::{Color, Grid, Sgr};
use crate::osc::{self, Event};

/// A parsed CSI sequence.
#[derive(Debug, Default)]
struct Csi {
    private: Option<u8>,
    params: Vec<u32>,
    intermediates: Vec<u8>,
}

impl Csi {
    fn reset(&mut self) {
        self.private = None;
        self.params.clear();
        self.intermediates.clear();
    }

    fn digit(&mut self, byte: u8) {
        if self.params.is_empty() {
            self.params.push(0);
        }
        if let Some(last) = self.params.last_mut() {
            *last = last
                .saturating_mul(10)
                .saturating_add(u32::from(byte - b'0'));
        }
    }

    fn separator(&mut self) {
        self.params.push(0);
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum State {
    #[default]
    Ground,
    Escape,
    EscapeOne,
    Csi,
    CsiIntermediate,
    Osc,
    OscEscape,
    Skip,
    SkipEscape,
}

/// A terminal: the grid plus the block model extracted from shell markers.
#[derive(Debug)]
pub struct Terminal {
    pub grid: Grid,
    pub blocks: Blocks,
    /// Semantic events produced since the last call to [`Terminal::take_events`].
    events: Vec<Event>,
    responses: Vec<u8>,
    state: State,
    csi: Csi,
    osc: Vec<u8>,
    utf8: Vec<u8>,
    last_char: Option<char>,
}

impl Terminal {
    pub fn new(cols: usize, rows: usize) -> Self {
        Self {
            grid: Grid::new(cols, rows),
            blocks: Blocks::new(),
            events: Vec::new(),
            responses: Vec::new(),
            state: State::Ground,
            csi: Csi::default(),
            osc: Vec::new(),
            utf8: Vec::new(),
            last_char: None,
        }
    }

    /// Feeds bytes and returns what must be written back to the program (device
    /// status reports, cursor position reports, primary/secondary DA).
    pub fn feed(&mut self, input: &[u8]) -> Vec<u8> {
        for &byte in input {
            self.byte(byte);
        }
        std::mem::take(&mut self.responses)
    }

    pub fn take_events(&mut self) -> Vec<Event> {
        std::mem::take(&mut self.events)
    }

    pub fn resize(&mut self, cols: usize, rows: usize) {
        self.grid.resize(cols, rows);
    }

    /// Writes text as if the program had printed it (used by tests and by the
    /// input echo in the reflow-free fallback mode).
    pub fn print(&mut self, text: &str) {
        self.grid.write_str(text);
    }

    fn byte(&mut self, byte: u8) {
        match self.state {
            State::Ground => self.ground(byte),
            State::Escape => self.escape(byte),
            State::EscapeOne => self.state = State::Ground,
            State::Csi => self.csi_byte(byte),
            State::CsiIntermediate => {
                if byte == 0x1b {
                    self.state = State::Escape;
                } else if (0x20..=0x2f).contains(&byte) {
                    self.csi.intermediates.push(byte);
                } else if (0x30..=0x3f).contains(&byte) {
                    self.csi_byte(byte);
                } else if (0x40..=0x7e).contains(&byte) {
                    self.dispatch_csi(byte);
                    self.state = State::Ground;
                } else {
                    self.state = State::Ground;
                }
            }
            State::Osc => match byte {
                0x07 | 0x9c => {
                    self.dispatch_osc();
                    self.state = State::Ground;
                }
                0x1b => self.state = State::OscEscape,
                _ => {
                    if self.osc.len() < 8192 {
                        self.osc.push(byte);
                    }
                }
            },
            State::OscEscape => {
                if byte == b'\\' {
                    self.dispatch_osc();
                    self.state = State::Ground;
                } else {
                    self.osc.clear();
                    self.state = State::Escape;
                    self.escape(byte);
                }
            }
            State::Skip => match byte {
                0x1b => self.state = State::SkipEscape,
                0x9c => self.state = State::Ground,
                _ => {}
            },
            State::SkipEscape => {
                if byte == b'\\' {
                    self.state = State::Ground;
                } else {
                    self.state = State::Escape;
                    self.escape(byte);
                }
            }
        }
    }

    fn ground(&mut self, byte: u8) {
        match byte {
            0x1b => self.state = State::Escape,
            0x9b => {
                self.csi.reset();
                self.state = State::Csi;
            }
            0x9d => {
                self.osc.clear();
                self.state = State::Osc;
            }
            0x00 => {}
            0x07 => {}
            0x08 => self.grid.backspace(),
            0x09 => self.grid.tab(),
            0x0a | 0x0b | 0x0c => self.grid.linefeed(),
            0x0d => self.grid.carriage_return(),
            0x7f => {}
            _ => self.utf8(byte),
        }
    }

    fn escape(&mut self, byte: u8) {
        match byte {
            b'[' => {
                self.csi.reset();
                self.state = State::Csi;
            }
            b']' => {
                self.osc.clear();
                self.state = State::Osc;
            }
            b'P' | b'X' | b'^' | b'_' => self.state = State::Skip,
            b'(' | b')' | b'*' | b'+' | b'#' | b'%' => self.state = State::EscapeOne,
            b'7' => {
                self.grid.save_cursor();
                self.state = State::Ground;
            }
            b'8' => {
                self.grid.restore_cursor();
                self.state = State::Ground;
            }
            b'D' => {
                self.grid.linefeed();
                self.state = State::Ground;
            }
            b'E' => {
                self.grid.newline();
                self.state = State::Ground;
            }
            b'M' => {
                self.grid.reverse_index();
                self.state = State::Ground;
            }
            b'c' => {
                self.grid.reset();
                self.state = State::Ground;
            }
            // Keypad modes, cursor-key modes and single shifts: accepted and
            // ignored, because applications assume they are supported.
            b'=' | b'>' | b'H' | b'Z' | b'\\' | 0x0e | 0x0f => self.state = State::Ground,
            0x1b => self.state = State::Escape,
            _ => self.state = State::Ground,
        }
    }

    fn csi_byte(&mut self, byte: u8) {
        match byte {
            // A new escape aborts the CSI, as in xterm and vte.
            0x1b => {
                self.state = State::Escape;
                return;
            }
            b'0'..=b'9' => {
                self.csi.digit(byte);
                self.state = State::Csi;
            }
            b';' | b':' => {
                self.csi.separator();
                self.state = State::Csi;
            }
            b'?' | b'>' | b'<' | b'=' => {
                if self.csi.params.is_empty() {
                    self.csi.private = Some(byte);
                }
                self.state = State::Csi;
            }
            b' '..=b'/' => {
                self.csi.intermediates.push(byte);
                self.state = State::CsiIntermediate;
            }
            0x40..=0x7e => {
                self.dispatch_csi(byte);
                self.state = State::Ground;
            }
            _ => {}
        }
    }

    fn dispatch_csi(&mut self, final_byte: u8) {
        let params = std::mem::take(&mut self.csi.params);
        let private = self.csi.private;
        let intermediates = std::mem::take(&mut self.csi.intermediates);

        // A missing or zero count means "one" for every movement command.
        let count = |index: usize, default: usize| {
            params
                .get(index)
                .map(|&value| usize::try_from(value).unwrap_or(default).max(1))
                .unwrap_or(default)
        };

        match final_byte {
            b'A' => self.grid.move_up(count(0, 1)),
            b'B' | b'e' => self.grid.move_down(count(0, 1)),
            b'C' | b'a' => self.grid.move_right(count(0, 1)),
            b'D' => self.grid.move_left(count(0, 1)),
            b'E' => {
                self.grid.move_down(count(0, 1));
                self.grid.carriage_return();
            }
            b'F' => {
                self.grid.move_up(count(0, 1));
                self.grid.carriage_return();
            }
            b'G' | b'`' => self.grid.set_col(count(0, 1) - 1),
            b'd' => self.grid.set_row(count(0, 1) - 1),
            b'H' | b'f' => {
                let row = count(0, 1) - 1;
                let col = count(1, 1) - 1;
                self.grid.set_cursor(col, row);
            }
            b'J' => self.grid.erase_display(first_u16(&params)),
            b'K' => self.grid.erase_line(first_u16(&params)),
            b'L' => self.grid.insert_lines(count(0, 1)),
            b'M' => self.grid.delete_lines(count(0, 1)),
            b'P' => self.grid.delete_chars(count(0, 1)),
            b'@' => self.grid.insert_chars(count(0, 1)),
            b'X' => self.grid.erase_chars(count(0, 1)),
            b'S' => self.grid.scroll_up(count(0, 1)),
            b'T' => self.grid.scroll_down(count(0, 1)),
            b'Z' => {
                for _ in 0..count(0, 1) {
                    self.grid.back_tab();
                }
            }
            b'b' => {
                if let Some(ch) = self.last_char {
                    for _ in 0..count(0, 1) {
                        self.grid.write_char(ch);
                    }
                }
            }
            b'm' => self.apply_sgr(&params),
            b'h' | b'l' => self.set_modes(private, &params, final_byte == b'h'),
            b'r' => {
                let top = count(0, 1) - 1;
                let bottom = state_param(&params, 1, self.grid.rows()) - 1;
                self.grid.set_scroll_region(top, bottom);
            }
            b's' => self.grid.save_cursor(),
            b'u' => self.grid.restore_cursor(),
            b'n' => self.device_status(&params),
            b'c' => self.device_attributes(private),
            _ => {
                let _ = intermediates;
            }
        }

        self.csi.reset();
        self.csi.private = None;
    }

    fn apply_sgr(&mut self, params: &[u32]) {
        let actions = resolve_sgr(params);
        self.grid.sgr(&actions);
    }

    fn set_modes(&mut self, private: Option<u8>, params: &[u32], enable: bool) {
        if private != Some(b'?') {
            if params.first() == Some(&4) {
                self.grid.insert_mode = enable;
            }
            return;
        }
        for &param in params {
            match param {
                1 => {} // application cursor keys: the input layer tracks it
                6 => {} // origin mode: not modeled, DECOM is rare in TUIs
                7 => self.grid.auto_wrap = enable,
                25 => self.grid.cursor_visible = enable,
                47 | 1047 | 1049 => {
                    if enable {
                        self.grid.enter_alt_screen();
                    } else {
                        self.grid.exit_alt_screen();
                    }
                }
                1000..=1006 | 1015 | 2004 | 2026 => {}
                _ => {}
            }
        }
    }

    fn device_status(&mut self, params: &[u32]) {
        match params.first() {
            Some(&5) => self.responses.extend_from_slice(b"\x1b[0n"),
            Some(&6) => {
                let cursor = self.grid.cursor();
                let report = format!("\x1b[{};{}R", cursor.y + 1, cursor.x + 1);
                self.responses.extend_from_slice(report.as_bytes());
            }
            _ => {}
        }
    }

    fn device_attributes(&mut self, private: Option<u8>) {
        if private == Some(b'>') {
            self.responses.extend_from_slice(b"\x1b[>0;276;0c");
        } else {
            self.responses.extend_from_slice(b"\x1b[?6c");
        }
    }

    fn dispatch_osc(&mut self) {
        let payload = String::from_utf8_lossy(&self.osc).into_owned();
        let mut parts = payload.splitn(2, ';');
        let code = parts.next().unwrap_or_default();
        let rest = parts.next().unwrap_or_default();
        if code == "0" || code == "2" {
            self.grid.set_title(rest);
        }
        if let Some(event) = osc::parse_payload(&payload) {
            self.blocks.apply(&event);
            self.events.push(event);
        }
    }

    fn utf8(&mut self, byte: u8) {
        self.utf8.push(byte);
        match std::str::from_utf8(&self.utf8) {
            Ok(text) => {
                let text = text.to_owned();
                self.utf8.clear();
                for ch in text.chars() {
                    self.last_char = Some(ch);
                    self.grid.write_char(ch);
                }
            }
            Err(error) if error.error_len().is_none() && self.utf8.len() < 4 => {}
            Err(_) => self.utf8.clear(),
        }
    }
}

fn first_u16(params: &[u32]) -> u16 {
    params
        .first()
        .copied()
        .unwrap_or(0)
        .min(u32::from(u16::MAX)) as u16
}

/// 1-based parameter with a default, clamped to a sensible range.
fn state_param(params: &[u32], index: usize, default: usize) -> usize {
    params
        .get(index)
        .map(|&value| {
            let value = usize::try_from(value).unwrap_or(default);
            if value == 0 { default } else { value }
        })
        .unwrap_or(default)
}

/// Expands a raw SGR parameter list, including 38/48 extended colors.
fn resolve_sgr(params: &[u32]) -> Vec<Sgr> {
    let mut actions = Vec::new();
    if params.is_empty() {
        actions.push(Sgr::Reset);
        return actions;
    }
    let mut index = 0;
    while index < params.len() {
        let param = params[index];
        match param {
            0 => actions.push(Sgr::Reset),
            1 => actions.push(Sgr::Bold),
            2 => actions.push(Sgr::Dim),
            3 => actions.push(Sgr::Italic),
            4 => actions.push(Sgr::Underline),
            5 | 6 => actions.push(Sgr::Blink),
            7 => actions.push(Sgr::Reverse),
            8 => actions.push(Sgr::Hidden),
            9 => actions.push(Sgr::Strike),
            21 | 22 => actions.push(Sgr::NoBold),
            23 => actions.push(Sgr::NoItalic),
            24 => actions.push(Sgr::NoUnderline),
            25 => actions.push(Sgr::NoBlink),
            27 => actions.push(Sgr::NoReverse),
            28 => actions.push(Sgr::NoHidden),
            29 => actions.push(Sgr::NoStrike),
            30..=37 => actions.push(Sgr::Fg(Color::Indexed((param - 30) as u8))),
            39 => actions.push(Sgr::FgDefault),
            40..=47 => actions.push(Sgr::Bg(Color::Indexed((param - 40) as u8))),
            49 => actions.push(Sgr::BgDefault),
            90..=97 => actions.push(Sgr::Fg(Color::Indexed((param - 90 + 8) as u8))),
            100..=107 => actions.push(Sgr::Bg(Color::Indexed((param - 100 + 8) as u8))),
            38 | 48 => {
                let is_fg = param == 38;
                let color = match params.get(index + 1).copied() {
                    Some(5) => params.get(index + 2).map(|&value| {
                        let value = value.min(255) as u8;
                        index += 2;
                        Color::Indexed(value)
                    }),
                    Some(2) => {
                        let red = params.get(index + 2).copied().unwrap_or(0).min(255) as u8;
                        let green = params.get(index + 3).copied().unwrap_or(0).min(255) as u8;
                        let blue = params.get(index + 4).copied().unwrap_or(0).min(255) as u8;
                        index += 4;
                        Some(Color::Rgb(red, green, blue))
                    }
                    _ => None,
                };
                if let Some(color) = color {
                    actions.push(if is_fg {
                        Sgr::Fg(color)
                    } else {
                        Sgr::Bg(color)
                    });
                }
            }
            _ => {}
        }
        index += 1;
    }
    actions
}
