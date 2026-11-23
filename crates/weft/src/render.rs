//! ANSI renderer: turns a [`Grid`] plus a status line into one frame of bytes.
//!
//! This is the dependency-free front-end. The GPU front-end (M1) consumes the
//! same grid and block model; only drawing changes. Frames are wrapped in the
//! synchronized-update sequence so a slow terminal never shows half a paint.

use std::fmt::Write;

use weft_term::grid::{Attrs, Color, Grid, Wide, char_width};

/// Renders the visible grid, the status bar and the cursor into `out`.
pub fn frame(grid: &Grid, status: &str, out: &mut String) {
    out.clear();
    out.push_str("\x1b[?2026h\x1b[H");
    let cols = grid.cols();
    let rows = grid.rows();
    let mut current = Style::default();

    for y in 0..rows {
        for x in 0..cols {
            let cell = grid.cell(x, y);
            if cell.wide == Wide::Trailing {
                continue;
            }
            let style = Style::of(cell.fg, cell.bg, cell.attrs);
            if style != current {
                style.write_sgr(out);
                current = style;
            }
            if cell.attrs.hidden {
                out.push(' ');
            } else {
                out.push(cell.ch);
            }
        }
        // A row never inherits attributes from the previous one, and the status
        // bar always starts on its own line.
        out.push_str("\x1b[0m\r\n");
        current = Style::default();
    }

    // Status bar: reverse video is available on every terminal worth targeting.
    out.push_str("\x1b[0;7m");
    pad(out, status, cols);
    out.push_str("\x1b[0m\x1b[J");

    if grid.cursor_visible {
        let cursor = grid.cursor();
        let _ = write!(out, "\x1b[{};{}H\x1b[?25h", cursor.y + 1, cursor.x + 1);
    } else {
        out.push_str("\x1b[?25l");
    }
    out.push_str("\x1b[?2026l");
}

fn pad(out: &mut String, text: &str, cols: usize) {
    let mut width = 0;
    for ch in text.chars() {
        let advance = char_width(ch);
        if width + advance > cols {
            break;
        }
        out.push(ch);
        width += advance;
    }
    for _ in width..cols {
        out.push(' ');
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct Style {
    fg: Color,
    bg: Color,
    attrs: Attrs,
}

impl Default for Style {
    fn default() -> Self {
        Self {
            fg: Color::Default,
            bg: Color::Default,
            attrs: Attrs::default(),
        }
    }
}

impl Style {
    fn of(fg: Color, bg: Color, attrs: Attrs) -> Self {
        let (fg, bg) = if attrs.reverse { (bg, fg) } else { (fg, bg) };
        Self { fg, bg, attrs }
    }

    fn write_sgr(&self, out: &mut String) {
        out.push_str("\x1b[0");
        if self.attrs.bold {
            out.push_str(";1");
        }
        if self.attrs.dim {
            out.push_str(";2");
        }
        if self.attrs.italic {
            out.push_str(";3");
        }
        if self.attrs.underline {
            out.push_str(";4");
        }
        if self.attrs.blink {
            out.push_str(";5");
        }
        if self.attrs.strike {
            out.push_str(";9");
        }
        push_color(out, self.fg, true);
        push_color(out, self.bg, false);
        out.push('m');
    }
}

fn push_color(out: &mut String, color: Color, foreground: bool) {
    match color {
        Color::Default => {}
        Color::Indexed(index) => {
            let base = if foreground { 38 } else { 48 };
            let _ = write!(out, ";{base};5;{index}");
        }
        Color::Rgb(red, green, blue) => {
            let base = if foreground { 38 } else { 48 };
            let _ = write!(out, ";{base};2;{red};{green};{blue}");
        }
    }
}
