//! ANSI renderer: turns a [`Grid`] plus a status line into one frame of bytes.
//!
//! This is the dependency-free front-end. The GPU front-end (M1) consumes the
//! same grid and block model; only drawing changes. Frames are wrapped in the
//! synchronized-update sequence so a slow terminal never shows half a paint.

use std::fmt::Write;

use weft_term::grid::{Attrs, Color, Grid, Wide, char_width};
use weft_term::layout::{Orientation, Segment};

use crate::theme::Theme;

/// One pane in a split layout.
#[derive(Clone, Copy)]
pub struct PaneView<'a> {
    /// Screen model of this pane.
    pub grid: &'a Grid,
    /// Whether this pane owns the cursor and receives input.
    pub active: bool,
    /// First row of the pane, zero-based.
    pub row: usize,
    /// First column of the pane, zero-based.
    pub column: usize,
}

/// Renders the panes of the active tab, separators between them and one status
/// bar.
///
/// Panes are addressed with absolute cursor positions, so each one keeps its
/// own scroll region and no pane can draw over the next.
pub fn split(
    views: &[PaneView<'_>],
    separators: &[Segment],
    status_row: usize,
    width: usize,
    status: &str,
    theme: &Theme,
    out: &mut String,
) {
    out.clear();
    out.push_str("\x1b[?2026h\x1b[H");
    if views.is_empty() {
        out.push_str("\x1b[?25l\x1b[?2026l");
        return;
    }
    let mut cursor = None;
    for view in views {
        write_rows(view, theme, out);
        if view.active {
            let position = view.grid.cursor();
            cursor = Some((
                view.row + position.y + 1,
                view.column + position.x + 1,
                view.grid.cursor_visible,
            ));
        }
    }
    for segment in separators {
        match segment.orientation {
            Orientation::Stacked => {
                separate_horizontal(segment.x, segment.y, segment.length, theme, out);
            }
            Orientation::SideBySide => {
                separate_vertical(segment.x, segment.y, segment.length, theme, out);
            }
        }
    }

    let (fr, fg, fb) = theme.foreground;
    let (br, bg, bb) = theme.background;
    let _ = write!(
        out,
        "\x1b[{};1H\x1b[0;38;2;{fr};{fg};{fb};48;2;{br};{bg};{bb}m",
        status_row + 1
    );
    pad(out, status, width);
    out.push_str("\x1b[0m\x1b[J");

    match cursor {
        Some((row, column, true)) => {
            let _ = write!(out, "\x1b[{row};{column}H\x1b[?25h");
        }
        _ => out.push_str("\x1b[?25l"),
    }
    out.push_str("\x1b[?2026l");
}

fn write_rows(view: &PaneView<'_>, theme: &Theme, out: &mut String) {
    for y in 0..view.grid.rows() {
        let _ = write!(out, "\x1b[{};{}H", view.row + y + 1, view.column + 1);
        let mut current = Style::default();
        for x in 0..view.grid.cols() {
            let cell = view.grid.cell(x, y);
            if cell.wide == Wide::Trailing {
                continue;
            }
            let style = Style::of(cell.fg, cell.bg, cell.attrs, theme);
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
        out.push_str("\x1b[0m");
    }
}

fn separator_style(theme: &Theme, out: &mut String) {
    let (red, green, blue) = theme.palette[8];
    let (br, bg, bb) = theme.background;
    let _ = write!(
        out,
        "\x1b[0;38;2;{red};{green};{blue};48;2;{br};{bg};{bb}m"
    );
}

fn separate_horizontal(x: usize, y: usize, length: usize, theme: &Theme, out: &mut String) {
    let _ = write!(out, "\x1b[{};{}H", y + 1, x + 1);
    separator_style(theme, out);
    for _ in 0..length {
        out.push('─');
    }
    out.push_str("\x1b[0m");
}

fn separate_vertical(x: usize, y: usize, length: usize, theme: &Theme, out: &mut String) {
    separator_style(theme, out);
    for row in 0..length {
        let _ = write!(out, "\x1b[{};{}H│", y + row + 1, x + 1);
    }
    out.push_str("\x1b[0m");
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
    fn of(fg: Color, bg: Color, attrs: Attrs, theme: &Theme) -> Self {
        let (fg, bg) = if attrs.reverse { (bg, fg) } else { (fg, bg) };
        Self {
            fg: resolve_color(fg, true, theme),
            bg: resolve_color(bg, false, theme),
            attrs,
        }
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

/// Draws a command palette over the top-left corner of the frame.
///
/// The frame has already positioned the cursor, so this function addresses rows
/// absolutely and hides the cursor again when it finishes.
pub fn palette(items: &[&str], selected: usize, theme: &Theme, cols: usize, out: &mut String) {
    let content = items
        .iter()
        .map(|item| text_width(item))
        .max()
        .unwrap_or(0)
        .max(12)
        .min(cols.saturating_sub(4));
    let width = content + 4;
    let border = format!("+{}+", "-".repeat(width - 2));
    let (fr, fg, fb) = theme.foreground;
    let (br, bg, bb) = theme.background;
    let normal = format!("\x1b[0;38;2;{fr};{fg};{fb};48;2;{br};{bg};{bb}m");
    let active = format!("\x1b[0;38;2;{br};{bg};{bb};48;2;{fr};{fg};{fb}m");

    let mut row = 1;
    let _ = write!(out, "\x1b[{row};1H{normal}{border}");
    for (index, item) in items.iter().enumerate() {
        row += 1;
        let marker = if index == selected { '>' } else { ' ' };
        let prefix = format!("|{marker} ");
        let room = width.saturating_sub(prefix.chars().count() + 1);
        let body = pad_right(item, room);
        let style = if index == selected { &active } else { &normal };
        let _ = write!(out, "\x1b[{row};1H{style}{prefix}{body}|");
    }
    row += 1;
    let _ = write!(out, "\x1b[{row};1H{normal}{border}\x1b[0m\x1b[?25l");
}

fn text_width(text: &str) -> usize {
    text.chars().map(char_width).sum()
}

fn pad_right(text: &str, width: usize) -> String {
    let mut out = String::with_capacity(width);
    let mut used = 0;
    for c in text.chars() {
        let advance = char_width(c);
        if used + advance > width {
            break;
        }
        out.push(c);
        used += advance;
    }
    out.extend(std::iter::repeat_n(' ', width.saturating_sub(used)));
    out
}

fn resolve_color(color: Color, foreground: bool, theme: &Theme) -> Color {
    match color {
        Color::Default => {
            let (red, green, blue) = if foreground {
                theme.foreground
            } else {
                theme.background
            };
            Color::Rgb(red, green, blue)
        }
        Color::Indexed(index) if index < 16 => {
            let (red, green, blue) = theme.palette[usize::from(index)];
            Color::Rgb(red, green, blue)
        }
        other => other,
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
