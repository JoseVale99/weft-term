//! Screen buffer: cells, cursor, scrollback, modes and the operations a
//! terminal program expects (SGR, erase, insert/delete, scroll regions).
//!
//! The grid is pure data. It knows nothing about bytes, PTYs or rendering, so
//! the whole terminal can be driven from tests in memory.

use std::collections::VecDeque;

/// Default number of lines kept above the visible screen.
pub const SCROLLBACK_LIMIT: usize = 10_000;

/// A color as the terminal understands it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Color {
    #[default]
    Default,
    Indexed(u8),
    Rgb(u8, u8, u8),
}

/// Visual attributes of a cell.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Attrs {
    pub bold: bool,
    pub dim: bool,
    pub italic: bool,
    pub underline: bool,
    pub blink: bool,
    pub reverse: bool,
    pub hidden: bool,
    pub strike: bool,
}

impl Attrs {
    pub fn is_plain(self) -> bool {
        self == Attrs::default()
    }
}

/// How a cell participates in a double-width character.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Wide {
    #[default]
    No,
    /// First column of a wide character.
    Leading,
    /// Second column, painted by the leading cell.
    Trailing,
}

/// One cell of the grid.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cell {
    pub ch: char,
    pub fg: Color,
    pub bg: Color,
    pub attrs: Attrs,
    pub wide: Wide,
}

impl Default for Cell {
    fn default() -> Self {
        Self {
            ch: ' ',
            fg: Color::Default,
            bg: Color::Default,
            attrs: Attrs::default(),
            wide: Wide::No,
        }
    }
}

impl Cell {
    /// Whether the cell paints background or a glyph.
    pub fn is_blank(&self) -> bool {
        self.ch == ' ' && self.bg == Color::Default && self.wide == Wide::No
    }
}

/// Cursor position and deferred wrap.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Cursor {
    pub x: usize,
    pub y: usize,
    /// The cursor is on the last column and the next glyph wraps.
    pub pending_wrap: bool,
}

/// Current drawing attributes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Pen {
    pub fg: Color,
    pub bg: Color,
    pub attrs: Attrs,
}

impl Pen {
    pub fn reset(&mut self) {
        *self = Pen::default();
    }
}

#[derive(Clone, Debug)]
struct Screen {
    cells: Vec<Cell>,
    cursor: Cursor,
}

impl Screen {
    fn new(cols: usize, rows: usize) -> Self {
        Self {
            cells: vec![Cell::default(); cols.max(1) * rows.max(1)],
            cursor: Cursor::default(),
        }
    }
}

/// Mutable terminal screen plus scrollback.
#[derive(Debug)]
pub struct Grid {
    cols: usize,
    rows: usize,
    main: Screen,
    alt: Screen,
    alt_active: bool,
    scrollback: VecDeque<Vec<Cell>>,
    scroll_top: usize,
    scroll_bottom: usize,
    pub pen: Pen,
    pub cursor_visible: bool,
    pub auto_wrap: bool,
    pub insert_mode: bool,
    pub title: String,
    saved: Pen,
    saved_cursor: Cursor,
}

impl Grid {
    pub fn new(cols: usize, rows: usize) -> Self {
        let cols = cols.max(1);
        let rows = rows.max(1);
        Self {
            cols,
            rows,
            main: Screen::new(cols, rows),
            alt: Screen::new(cols, rows),
            alt_active: false,
            scrollback: VecDeque::new(),
            scroll_top: 0,
            scroll_bottom: rows - 1,
            pen: Pen::default(),
            cursor_visible: true,
            auto_wrap: true,
            insert_mode: false,
            title: String::new(),
            saved: Pen::default(),
            saved_cursor: Cursor::default(),
        }
    }

    pub fn cols(&self) -> usize {
        self.cols
    }

    pub fn rows(&self) -> usize {
        self.rows
    }

    pub fn alt_active(&self) -> bool {
        self.alt_active
    }

    pub fn scrollback_len(&self) -> usize {
        self.scrollback.len()
    }

    pub fn cursor(&self) -> Cursor {
        self.screen().cursor
    }

    pub fn scrollback(&self) -> impl DoubleEndedIterator<Item = &[Cell]> {
        self.scrollback.iter().map(Vec::as_slice)
    }

    fn screen(&self) -> &Screen {
        if self.alt_active {
            &self.alt
        } else {
            &self.main
        }
    }

    fn screen_mut(&mut self) -> &mut Screen {
        if self.alt_active {
            &mut self.alt
        } else {
            &mut self.main
        }
    }

    /// Cell at `(x, y)`, or a blank cell when out of bounds.
    pub fn cell(&self, x: usize, y: usize) -> Cell {
        if x >= self.cols || y >= self.rows {
            return Cell::default();
        }
        self.screen().cells[y * self.cols + x]
    }

    pub fn row(&self, y: usize) -> &[Cell] {
        let start = y.min(self.rows.saturating_sub(1)) * self.cols;
        &self.screen().cells[start..start + self.cols]
    }

    pub fn resize(&mut self, cols: usize, rows: usize) {
        let cols = cols.max(1);
        let rows = rows.max(1);
        if cols == self.cols && rows == self.rows {
            return;
        }
        let old = std::mem::replace(self, Grid::new(cols, rows));
        // Keep the scrollback and both screens, cropping or padding them.
        self.scrollback = old.scrollback;
        self.main = Self::resized(&old.main, old.cols, old.rows, cols, rows);
        self.alt = Self::resized(&old.alt, old.cols, old.rows, cols, rows);
        self.alt_active = old.alt_active;
        self.pen = old.pen;
        self.cursor_visible = old.cursor_visible;
        self.auto_wrap = old.auto_wrap;
        self.insert_mode = old.insert_mode;
        self.title = old.title;
        self.saved = old.saved;
        self.saved_cursor = old.saved_cursor;
        self.scroll_top = 0;
        self.scroll_bottom = rows - 1;
    }

    fn resized(
        screen: &Screen,
        old_cols: usize,
        old_rows: usize,
        cols: usize,
        rows: usize,
    ) -> Screen {
        let mut next = Screen::new(cols, rows);
        let copy_rows = old_rows.min(rows);
        let copy_cols = old_cols.min(cols);
        for y in 0..copy_rows {
            for x in 0..copy_cols {
                next.cells[y * cols + x] = screen.cells[y * old_cols + x];
            }
        }
        next.cursor.x = screen.cursor.x.min(cols - 1);
        next.cursor.y = screen.cursor.y.min(rows - 1);
        next.cursor.pending_wrap = false;
        next
    }

    pub fn reset(&mut self) {
        let (cols, rows) = (self.cols, self.rows);
        *self = Grid::new(cols, rows);
    }

    pub fn set_title(&mut self, title: impl Into<String>) {
        self.title = title.into();
    }

    // ---------------------------------------------------------------- writing

    pub fn write_char(&mut self, ch: char) {
        let width = char_width(ch);
        if width == 0 {
            self.combine(ch);
            return;
        }
        if self.screen().cursor.pending_wrap && self.auto_wrap {
            self.screen_mut().cursor.pending_wrap = false;
            self.linefeed();
            self.carriage_return();
        }
        if width == 2 && self.screen().cursor.x + 2 > self.cols {
            if self.auto_wrap {
                self.linefeed();
                self.carriage_return();
            } else {
                return;
            }
        }
        if self.insert_mode {
            self.insert_chars(width);
        }
        let (x, y) = {
            let cursor = self.screen().cursor;
            (cursor.x, cursor.y)
        };
        let cell = Cell {
            ch,
            fg: self.pen.fg,
            bg: self.pen.bg,
            attrs: self.pen.attrs,
            wide: if width == 2 { Wide::Leading } else { Wide::No },
        };
        let cols = self.cols;
        let index = y * cols + x;
        let screen = self.screen_mut();
        screen.cells[index] = cell;
        if width == 2 {
            screen.cells[index + 1] = Cell {
                wide: Wide::Trailing,
                ..Cell::default()
            };
        }
        let mut cursor = self.screen().cursor;
        cursor.x += width;
        if cursor.x >= self.cols {
            cursor.x = self.cols - 1;
            cursor.pending_wrap = true;
        }
        self.screen_mut().cursor = cursor;
    }

    /// Appends a combining mark to the previous cell when possible.
    fn combine(&mut self, ch: char) {
        let cursor = self.screen().cursor;
        let mut x = cursor.x;
        if cursor.pending_wrap {
            if x == 0 {
                return;
            }
            x -= 1;
        } else if x == 0 {
            return;
        } else {
            x -= 1;
        }
        let cols = self.cols;
        let screen = self.screen_mut();
        let index = cursor.y * cols + x;
        // A trailing half of a wide character belongs to its leading cell.
        let index = if screen.cells[index].wide == Wide::Trailing && x > 0 {
            index - 1
        } else {
            index
        };
        // A cell stores one base glyph. Combining marks are dropped for now;
        // grapheme clusters are a renderer concern from M1 on.
        if screen.cells[index].ch == ' ' {
            screen.cells[index].ch = ch;
        }
    }

    pub fn write_str(&mut self, text: &str) {
        for ch in text.chars() {
            self.write_char(ch);
        }
    }

    pub fn newline(&mut self) {
        self.linefeed();
        self.carriage_return();
    }

    pub fn linefeed(&mut self) {
        let mut cursor = self.screen().cursor;
        cursor.pending_wrap = false;
        if cursor.y == self.scroll_bottom {
            self.screen_mut().cursor = cursor;
            self.scroll_up(1);
        } else if cursor.y + 1 < self.rows {
            cursor.y += 1;
            self.screen_mut().cursor = cursor;
        } else {
            self.screen_mut().cursor = cursor;
        }
    }

    pub fn reverse_index(&mut self) {
        let mut cursor = self.screen().cursor;
        cursor.pending_wrap = false;
        if cursor.y == self.scroll_top {
            self.screen_mut().cursor = cursor;
            self.scroll_down(1);
        } else if cursor.y > 0 {
            cursor.y -= 1;
            self.screen_mut().cursor = cursor;
        } else {
            self.screen_mut().cursor = cursor;
        }
    }

    pub fn carriage_return(&mut self) {
        self.screen_mut().cursor.x = 0;
        self.screen_mut().cursor.pending_wrap = false;
    }

    pub fn backspace(&mut self) {
        let mut cursor = self.screen().cursor;
        cursor.pending_wrap = false;
        cursor.x = cursor.x.saturating_sub(1);
        self.screen_mut().cursor = cursor;
    }

    pub fn tab(&mut self) {
        let next = ((self.screen().cursor.x / 8) + 1) * 8;
        self.screen_mut().cursor.x = next.min(self.cols - 1);
        self.screen_mut().cursor.pending_wrap = false;
    }

    pub fn back_tab(&mut self) {
        let x = self.screen().cursor.x;
        self.screen_mut().cursor.x = if x % 8 == 0 {
            x.saturating_sub(8)
        } else {
            x - (x % 8)
        };
        self.screen_mut().cursor.pending_wrap = false;
    }

    // ---------------------------------------------------------------- cursor

    pub fn set_cursor(&mut self, x: usize, y: usize) {
        let mut cursor = self.screen().cursor;
        cursor.x = x.min(self.cols - 1);
        cursor.y = y.min(self.rows - 1);
        cursor.pending_wrap = false;
        self.screen_mut().cursor = cursor;
    }

    pub fn set_col(&mut self, x: usize) {
        let y = self.screen().cursor.y;
        self.set_cursor(x, y);
    }

    pub fn set_row(&mut self, y: usize) {
        let x = self.screen().cursor.x;
        self.set_cursor(x, y);
    }

    pub fn move_up(&mut self, n: usize) {
        let mut cursor = self.screen().cursor;
        cursor.y = cursor.y.saturating_sub(n);
        cursor.pending_wrap = false;
        self.screen_mut().cursor = cursor;
    }

    pub fn move_down(&mut self, n: usize) {
        let cursor = self.screen().cursor;
        let y = (cursor.y + n).min(self.scroll_bottom);
        self.set_cursor(cursor.x, y);
    }

    pub fn move_left(&mut self, n: usize) {
        let mut cursor = self.screen().cursor;
        cursor.x = cursor.x.saturating_sub(n);
        cursor.pending_wrap = false;
        self.screen_mut().cursor = cursor;
    }

    pub fn move_right(&mut self, n: usize) {
        let cursor = self.screen().cursor;
        let x = (cursor.x + n).min(self.cols - 1);
        self.set_cursor(x, cursor.y);
    }

    pub fn save_cursor(&mut self) {
        self.saved = self.pen;
        self.saved_cursor = self.screen().cursor;
    }

    pub fn restore_cursor(&mut self) {
        self.pen = self.saved;
        let cursor = Cursor {
            x: self.saved_cursor.x.min(self.cols - 1),
            y: self.saved_cursor.y.min(self.rows - 1),
            pending_wrap: false,
        };
        self.screen_mut().cursor = cursor;
    }

    // ---------------------------------------------------------------- scrolling

    pub fn set_scroll_region(&mut self, top: usize, bottom: usize) {
        if top < bottom && bottom < self.rows {
            self.scroll_top = top;
            self.scroll_bottom = bottom;
            self.set_cursor(0, top);
        }
    }

    pub fn reset_scroll_region(&mut self) {
        self.scroll_top = 0;
        self.scroll_bottom = self.rows - 1;
    }

    pub fn scroll_up(&mut self, n: usize) {
        let n = n.max(1).min(self.rows);
        for _ in 0..n {
            self.scroll_up_once();
        }
    }

    fn scroll_up_once(&mut self) {
        let cols = self.cols;
        let top = self.scroll_top;
        let bottom = self.scroll_bottom;
        let full = top == 0 && bottom + 1 == self.rows && !self.alt_active;
        if full {
            let line = self.screen().cells[0..cols].to_vec();
            self.scrollback.push_back(line);
            while self.scrollback.len() > SCROLLBACK_LIMIT {
                self.scrollback.pop_front();
            }
        }
        let start = (top + 1) * cols;
        let end = (bottom + 1) * cols;
        let screen = self.screen_mut();
        screen.cells.copy_within(start..end, top * cols);
        for cell in &mut screen.cells[bottom * cols..(bottom + 1) * cols] {
            *cell = Cell::default();
        }
    }

    pub fn scroll_down(&mut self, n: usize) {
        let n = n.max(1).min(self.rows);
        for _ in 0..n {
            let cols = self.cols;
            let top = self.scroll_top;
            let bottom = self.scroll_bottom;
            let screen = self.screen_mut();
            let start = top * cols;
            let end = bottom * cols;
            screen.cells.copy_within(start..end, (top + 1) * cols);
            for cell in &mut screen.cells[top * cols..(top + 1) * cols] {
                *cell = Cell::default();
            }
        }
    }

    pub fn insert_lines(&mut self, n: usize) {
        let y = self.screen().cursor.y;
        if y < self.scroll_top || y > self.scroll_bottom {
            return;
        }
        let n = n.max(1).min(self.scroll_bottom - y + 1);
        let (top, bottom) = (self.scroll_top, self.scroll_bottom);
        self.scroll_top = y;
        self.scroll_bottom = bottom;
        self.scroll_down(n);
        self.scroll_top = top;
    }

    pub fn delete_lines(&mut self, n: usize) {
        let y = self.screen().cursor.y;
        if y < self.scroll_top || y > self.scroll_bottom {
            return;
        }
        let n = n.max(1).min(self.scroll_bottom - y + 1);
        let (top, bottom) = (self.scroll_top, self.scroll_bottom);
        self.scroll_top = y;
        self.scroll_bottom = bottom;
        self.scroll_up(n);
        self.scroll_top = top;
    }

    pub fn insert_chars(&mut self, n: usize) {
        let n = n.max(1);
        let (x, y) = {
            let cursor = self.screen().cursor;
            (cursor.x, cursor.y)
        };
        let cols = self.cols;
        let screen = self.screen_mut();
        let start = y * cols + x;
        let row_end = (y + 1) * cols;
        let n = n.min(row_end - start);
        screen.cells.copy_within(start..row_end - n, start + n);
        for cell in &mut screen.cells[start..start + n] {
            *cell = Cell::default();
        }
    }

    pub fn delete_chars(&mut self, n: usize) {
        let n = n.max(1);
        let (x, y) = {
            let cursor = self.screen().cursor;
            (cursor.x, cursor.y)
        };
        let cols = self.cols;
        let screen = self.screen_mut();
        let start = y * cols + x;
        let row_end = (y + 1) * cols;
        let n = n.min(row_end - start);
        screen.cells.copy_within(start + n..row_end, start);
        for cell in &mut screen.cells[row_end - n..row_end] {
            *cell = Cell::default();
        }
    }

    pub fn erase_chars(&mut self, n: usize) {
        let n = n.max(1);
        let (x, y) = {
            let cursor = self.screen().cursor;
            (cursor.x, cursor.y)
        };
        let cols = self.cols;
        let blank = Cell {
            fg: self.pen.fg,
            bg: self.pen.bg,
            ..Cell::default()
        };
        let screen = self.screen_mut();
        let start = y * cols + x;
        let end = (start + n).min((y + 1) * cols);
        for cell in &mut screen.cells[start..end] {
            *cell = blank;
        }
    }

    // ---------------------------------------------------------------- erase

    pub fn erase_display(&mut self, mode: u16) {
        if mode == 3 {
            self.scrollback.clear();
            return;
        }
        let (x, y) = {
            let cursor = self.screen().cursor;
            (cursor.x, cursor.y)
        };
        let cols = self.cols;
        let rows = self.rows;
        let blank = Cell {
            fg: self.pen.fg,
            bg: self.pen.bg,
            ..Cell::default()
        };
        let screen = self.screen_mut();
        match mode {
            0 => {
                let start = y * cols + x;
                for cell in &mut screen.cells[start..] {
                    *cell = blank;
                }
            }
            1 => {
                let end = (y * cols + x + 1).min(rows * cols);
                for cell in &mut screen.cells[..end] {
                    *cell = blank;
                }
            }
            _ => {
                for cell in &mut screen.cells {
                    *cell = blank;
                }
            }
        }
    }

    pub fn erase_line(&mut self, mode: u16) {
        let (x, y) = {
            let cursor = self.screen().cursor;
            (cursor.x, cursor.y)
        };
        let cols = self.cols;
        let blank = Cell {
            fg: self.pen.fg,
            bg: self.pen.bg,
            ..Cell::default()
        };
        let screen = self.screen_mut();
        let start = y * cols;
        let end = start + cols;
        match mode {
            0 => {
                for cell in &mut screen.cells[start + x..end] {
                    *cell = blank;
                }
            }
            1 => {
                for cell in &mut screen.cells[start..=start + x] {
                    *cell = blank;
                }
            }
            _ => {
                for cell in &mut screen.cells[start..end] {
                    *cell = blank;
                }
            }
        }
    }

    // ---------------------------------------------------------------- modes

    /// Switches to the alternate screen, saving the cursor and clearing the
    /// alternate buffer the way xterm does for `CSI ? 1049 h`.
    pub fn enter_alt_screen(&mut self) {
        if self.alt_active {
            return;
        }
        self.saved_cursor = self.screen().cursor;
        self.alt_active = true;
        self.alt = Screen::new(self.cols, self.rows);
        self.set_cursor(0, 0);
    }

    pub fn exit_alt_screen(&mut self) {
        if !self.alt_active {
            return;
        }
        self.alt_active = false;
        self.restore_cursor();
    }

    /// Applies one SGR parameter list. `params` is already tokenized and
    /// expanded (38/48 sequences resolved by the parser).
    pub fn sgr(&mut self, params: &[Sgr]) {
        for param in params {
            match param {
                Sgr::Reset => self.pen.reset(),
                Sgr::Bold => self.pen.attrs.bold = true,
                Sgr::Dim => self.pen.attrs.dim = true,
                Sgr::Italic => self.pen.attrs.italic = true,
                Sgr::Underline => self.pen.attrs.underline = true,
                Sgr::Blink => self.pen.attrs.blink = true,
                Sgr::Reverse => self.pen.attrs.reverse = true,
                Sgr::Hidden => self.pen.attrs.hidden = true,
                Sgr::Strike => self.pen.attrs.strike = true,
                Sgr::NoBold => {
                    self.pen.attrs.bold = false;
                    self.pen.attrs.dim = false;
                }
                Sgr::NoItalic => self.pen.attrs.italic = false,
                Sgr::NoUnderline => self.pen.attrs.underline = false,
                Sgr::NoBlink => self.pen.attrs.blink = false,
                Sgr::NoReverse => self.pen.attrs.reverse = false,
                Sgr::NoHidden => self.pen.attrs.hidden = false,
                Sgr::NoStrike => self.pen.attrs.strike = false,
                Sgr::Fg(color) => self.pen.fg = *color,
                Sgr::Bg(color) => self.pen.bg = *color,
                Sgr::FgDefault => self.pen.fg = Color::Default,
                Sgr::BgDefault => self.pen.bg = Color::Default,
            }
        }
    }
}

/// A single resolved SGR action.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sgr {
    Reset,
    Bold,
    Dim,
    Italic,
    Underline,
    Blink,
    Reverse,
    Hidden,
    Strike,
    NoBold,
    NoItalic,
    NoUnderline,
    NoBlink,
    NoReverse,
    NoHidden,
    NoStrike,
    Fg(Color),
    Bg(Color),
    FgDefault,
    BgDefault,
}

/// Display width of a character in terminal cells.
///
/// Zero for control characters and combining marks, two for East Asian wide and
/// emoji, one otherwise. The table covers the ranges that appear in practice;
/// the full Unicode width tables belong to the renderer at M1.
pub fn char_width(ch: char) -> usize {
    let cp = ch as u32;
    if cp < 0x20 || (0x7f..0xa0).contains(&cp) {
        return 0;
    }
    if is_combining(cp) {
        return 0;
    }
    if is_wide(cp) { 2 } else { 1 }
}

fn is_combining(cp: u32) -> bool {
    matches!(cp,
        0x0300..=0x036f
        | 0x0483..=0x0489
        | 0x0591..=0x05bd
        | 0x0610..=0x061a
        | 0x064b..=0x065f
        | 0x0670
        | 0x06d6..=0x06dc
        | 0x0e31
        | 0x0e34..=0x0e3a
        | 0x0e47..=0x0e4e
        | 0x1ab0..=0x1aff
        | 0x1dc0..=0x1dff
        | 0x20d0..=0x20ff
        | 0xfe00..=0xfe0f
        | 0xfe20..=0xfe2f
        | 0xe0100..=0xe01ef)
}

fn is_wide(cp: u32) -> bool {
    matches!(cp,
        0x1100..=0x115f
        | 0x2e80..=0x303e
        | 0x3041..=0x33ff
        | 0x3400..=0x4dbf
        | 0x4e00..=0x9fff
        | 0xa000..=0xa4cf
        | 0xa960..=0xa97f
        | 0xac00..=0xd7a3
        | 0xf900..=0xfaff
        | 0xfe10..=0xfe19
        | 0xfe30..=0xfe4f
        | 0xff00..=0xff60
        | 0xffe0..=0xffe6
        | 0x1f300..=0x1f64f
        | 0x1f900..=0x1f9ff
        | 0x1fa70..=0x1faff
        | 0x20000..=0x3fffd)
}
