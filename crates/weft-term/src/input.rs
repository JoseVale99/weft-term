//! The input line editor.
//!
//! It is a pure text buffer: no terminal, no key codes, no rendering. The
//! front-end decodes keys into operations, asks for the text and draws it. That
//! keeps every editing rule testable without a real terminal, and lets the same
//! editor serve the ANSI front-end today and the GPU window later.

use unicode_segmentation::UnicodeSegmentation;

/// Result of a completion request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Completion {
    /// The buffer changed; redraw and send another `Tab` to keep cycling.
    Inserted,
    /// Several candidates share no longer prefix. Show them to the user.
    Ambiguous(Vec<String>),
    /// Nothing matched the current word.
    None,
}

/// A multiline text buffer with a grapheme-aware cursor and a kill ring.
#[derive(Debug, Clone, Default)]
pub struct Editor {
    text: String,
    cursor: usize,
    /// Column the user is aiming for while moving vertically.
    desired_column: Option<usize>,
    kills: Vec<String>,
}

impl Editor {
    /// Creates an empty editor.
    pub fn new() -> Self {
        Self::default()
    }

    /// Current text.
    pub fn text(&self) -> &str {
        &self.text
    }

    /// Cursor as a byte offset on a grapheme boundary.
    pub fn cursor(&self) -> usize {
        self.cursor
    }

    /// Whether the buffer is empty.
    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }

    /// Replaces the whole buffer and leaves the cursor at the end.
    pub fn set_text(&mut self, text: impl Into<String>) {
        self.text = sanitize(&text.into());
        self.cursor = self.text.len();
        self.desired_column = None;
    }

    /// Empties the buffer and returns what it held.
    pub fn take(&mut self) -> String {
        self.cursor = 0;
        self.desired_column = None;
        std::mem::take(&mut self.text)
    }

    /// Inserts text at the cursor.
    ///
    /// Control characters are dropped except newline and tab, so a paste cannot
    /// inject escape sequences into the terminal.
    pub fn insert(&mut self, text: &str) {
        let text = sanitize(text);
        self.text.insert_str(self.cursor, &text);
        self.cursor += text.len();
        self.desired_column = None;
    }

    /// Inserts a line break.
    pub fn insert_newline(&mut self) {
        self.insert("\n");
    }

    /// Removes the grapheme before the cursor.
    pub fn backspace(&mut self) {
        if let Some((start, _)) = self.previous_grapheme() {
            self.text.replace_range(start..self.cursor, "");
            self.cursor = start;
            self.desired_column = None;
        }
    }

    /// Removes the grapheme after the cursor.
    pub fn delete_forward(&mut self) {
        if let Some((_, end)) = self.next_grapheme() {
            self.text.replace_range(self.cursor..end, "");
            self.desired_column = None;
        }
    }

    /// Moves one grapheme to the left.
    pub fn move_left(&mut self) {
        if let Some((start, _)) = self.previous_grapheme() {
            self.cursor = start;
        }
        self.desired_column = None;
    }

    /// Moves one grapheme to the right.
    pub fn move_right(&mut self) {
        if let Some((_, end)) = self.next_grapheme() {
            self.cursor = end;
        }
        self.desired_column = None;
    }

    /// Moves to the start of the previous word.
    pub fn move_word_left(&mut self) {
        self.cursor = word_start(&self.text, self.cursor);
        self.desired_column = None;
    }

    /// Moves past the end of the next word.
    pub fn move_word_right(&mut self) {
        self.cursor = word_end(&self.text, self.cursor);
        self.desired_column = None;
    }

    /// Moves to the start of the current visual line.
    pub fn move_home(&mut self) {
        self.cursor = line_start(&self.text, self.cursor);
        self.desired_column = None;
    }

    /// Moves to the end of the current visual line.
    pub fn move_end(&mut self) {
        self.cursor = line_end(&self.text, self.cursor);
        self.desired_column = None;
    }

    /// Moves to the beginning of the buffer.
    pub fn move_buffer_start(&mut self) {
        self.cursor = 0;
        self.desired_column = None;
    }

    /// Moves to the end of the buffer.
    pub fn move_buffer_end(&mut self) {
        self.cursor = self.text.len();
        self.desired_column = None;
    }

    /// Moves one visual line up, keeping the column when possible.
    pub fn move_up(&mut self) {
        let (row, column) = self.line_column();
        let column = self.desired_column.unwrap_or(column);
        self.desired_column = Some(column);
        self.cursor = if row == 0 {
            0
        } else {
            byte_at(&self.text, row - 1, column)
        };
    }

    /// Moves one visual line down, keeping the column when possible.
    pub fn move_down(&mut self) {
        let (row, column) = self.line_column();
        let column = self.desired_column.unwrap_or(column);
        self.desired_column = Some(column);
        let rows = self.rows();
        self.cursor = if row + 1 >= rows {
            self.text.len()
        } else {
            byte_at(&self.text, row + 1, column)
        };
    }

    /// Deletes from the cursor to the end of the line, or joins lines at the end.
    pub fn kill_to_end(&mut self) {
        let end = line_end(&self.text, self.cursor);
        let end = if end == self.cursor {
            (end + 1).min(self.text.len())
        } else {
            end
        };
        self.kill(self.cursor..end);
    }

    /// Deletes from the start of the line to the cursor.
    pub fn kill_to_start(&mut self) {
        let start = line_start(&self.text, self.cursor);
        self.kill(start..self.cursor);
    }

    /// Deletes the previous word.
    pub fn kill_word_back(&mut self) {
        let start = word_start(&self.text, self.cursor);
        self.kill(start..self.cursor);
    }

    /// Deletes the next word.
    pub fn kill_word_forward(&mut self) {
        let end = word_end(&self.text, self.cursor);
        self.kill(self.cursor..end);
    }

    /// Inserts the most recent killed text.
    pub fn yank(&mut self) {
        if let Some(text) = self.kills.first().cloned() {
            self.insert(&text);
        }
    }

    /// Row and character column of the cursor, both zero-based.
    pub fn line_column(&self) -> (usize, usize) {
        let row = self.text[..self.cursor].matches('\n').count();
        let start = line_start(&self.text, self.cursor);
        let column = self.text[start..self.cursor].chars().count();
        (row, column)
    }

    /// Number of visual lines, at least one.
    pub fn rows(&self) -> usize {
        self.text.matches('\n').count() + 1
    }

    /// The word being completed: everything since the last whitespace.
    pub fn word(&self) -> &str {
        let start = word_start(&self.text, self.cursor);
        &self.text[start..self.cursor]
    }

    /// Inserts the unique completion for the current word, or its longest
    /// common prefix when there are several candidates.
    pub fn complete(&mut self, candidates: &[String]) -> Completion {
        let word = self.word();
        let matching: Vec<&String> = candidates
            .iter()
            .filter(|candidate| candidate.starts_with(word))
            .collect();
        if matching.is_empty() {
            return Completion::None;
        }
        if let [only] = matching.as_slice() {
            self.insert(&only[word.len()..]);
            return Completion::Inserted;
        }
        let prefix = common_prefix(&matching);
        if prefix.len() > word.len() {
            self.insert(&prefix[word.len()..]);
            return Completion::Inserted;
        }
        Completion::Ambiguous(matching.into_iter().cloned().collect())
    }

    fn previous_grapheme(&self) -> Option<(usize, usize)> {
        self.text[..self.cursor]
            .grapheme_indices(true)
            .next_back()
            .map(|(start, grapheme)| (start, start + grapheme.len()))
    }

    fn next_grapheme(&self) -> Option<(usize, usize)> {
        self.text[self.cursor..]
            .grapheme_indices(true)
            .next()
            .map(|(offset, grapheme)| {
                (self.cursor + offset, self.cursor + offset + grapheme.len())
            })
    }

    fn kill(&mut self, range: std::ops::Range<usize>) {
        if range.is_empty() {
            return;
        }
        let killed = self.text[range.clone()].to_owned();
        self.text.replace_range(range.clone(), "");
        self.cursor = range.start;
        self.desired_column = None;
        self.kills.insert(0, killed);
        self.kills.truncate(16);
    }
}

fn sanitize(text: &str) -> String {
    text.chars()
        .filter(|c| *c == '\n' || *c == '\t' || !c.is_control())
        .collect()
}

fn line_start(text: &str, at: usize) -> usize {
    text[..at].rfind('\n').map_or(0, |index| index + 1)
}

fn line_end(text: &str, at: usize) -> usize {
    text[at..].find('\n').map_or(text.len(), |index| at + index)
}

fn word_start(text: &str, at: usize) -> usize {
    let mut chars = text[..at].char_indices().rev().peekable();
    while chars.peek().is_some_and(|(_, c)| c.is_whitespace()) {
        chars.next();
    }
    let mut start = at;
    while chars.peek().is_some_and(|(_, c)| !c.is_whitespace()) {
        start = chars.next().map_or(start, |(index, _)| index);
    }
    start
}

fn word_end(text: &str, at: usize) -> usize {
    let mut chars = text[at..].char_indices().peekable();
    while chars.peek().is_some_and(|(_, c)| c.is_whitespace()) {
        chars.next();
    }
    while chars.peek().is_some_and(|(_, c)| !c.is_whitespace()) {
        chars.next();
    }
    while chars.peek().is_some_and(|(_, c)| c.is_whitespace()) {
        chars.next();
    }
    chars.peek().map_or(text.len(), |(index, _)| at + index)
}

fn byte_at(text: &str, row: usize, column: usize) -> usize {
    let mut start = 0;
    for _ in 0..row {
        let Some(offset) = text[start..].find('\n') else {
            return text.len();
        };
        start += offset + 1;
    }
    let line = &text[start..line_end(text, start)];
    line.char_indices()
        .nth(column)
        .map_or(start + line.len(), |(index, _)| start + index)
}

fn common_prefix(candidates: &[&String]) -> String {
    let Some(first) = candidates.first() else {
        return String::new();
    };
    let mut prefix = String::new();
    for (index, c) in first.char_indices() {
        let next = index + c.len_utf8();
        if candidates
            .iter()
            .all(|candidate| candidate[..next] == first[..next])
        {
            prefix.push(c);
        } else {
            break;
        }
    }
    prefix
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inserts_at_the_cursor() {
        let mut editor = Editor::new();
        editor.insert("abcd");
        editor.move_left();
        editor.move_left();
        editor.insert("XY");
        assert_eq!(editor.text(), "abXYcd");
        assert_eq!(editor.cursor(), 4);
    }

    #[test]
    fn editing_works_on_multibyte_text() {
        let mut editor = Editor::new();
        editor.insert("añ");
        assert_eq!(editor.text(), "añ");
        editor.backspace();
        assert_eq!(editor.text(), "a");
        editor.insert("é");
        editor.backspace();
        editor.backspace();
        assert_eq!(editor.text(), "");
    }

    #[test]
    fn backspace_removes_a_whole_grapheme() {
        let mut editor = Editor::new();
        editor.insert("e\u{0301}");
        editor.backspace();
        assert_eq!(editor.text(), "");
    }

    #[test]
    fn control_characters_are_dropped_from_paste() {
        let mut editor = Editor::new();
        editor.insert("ls\u{1b}[31m");
        assert_eq!(editor.text(), "ls[31m");
    }

    #[test]
    fn word_movement_skips_spaces() {
        let mut editor = Editor::new();
        editor.insert("cargo test --all");
        editor.move_word_left();
        assert_eq!(&editor.text()[editor.cursor()..], "--all");
        editor.move_word_left();
        assert_eq!(&editor.text()[editor.cursor()..], "test --all");
        editor.move_word_right();
        assert_eq!(&editor.text()[editor.cursor()..], "--all");
    }

    #[test]
    fn home_and_end_stay_on_the_visual_line() {
        let mut editor = Editor::new();
        editor.insert("one\ntwo");
        editor.move_up();
        editor.move_home();
        assert_eq!(editor.cursor(), 0);
        editor.move_end();
        assert_eq!(editor.cursor(), 3);
    }

    #[test]
    fn vertical_movement_keeps_the_desired_column() {
        let mut editor = Editor::new();
        editor.insert("abcdef\nx\nlonger");
        assert_eq!(editor.line_column(), (2, 6));
        editor.move_up();
        assert_eq!(editor.line_column(), (1, 1));
        editor.move_up();
        assert_eq!(editor.line_column(), (0, 6));
        editor.move_down();
        assert_eq!(editor.line_column(), (1, 1));
    }

    #[test]
    fn kill_and_yank_round_trip() {
        let mut editor = Editor::new();
        editor.insert("git status");
        editor.kill_word_back();
        assert_eq!(editor.text(), "git ");
        editor.yank();
        assert_eq!(editor.text(), "git status");
    }

    #[test]
    fn kill_to_end_joins_lines_at_the_end() {
        let mut editor = Editor::new();
        editor.insert("one\ntwo");
        editor.move_up();
        editor.move_end();
        editor.kill_to_end();
        assert_eq!(editor.text(), "onetwo");
    }

    #[test]
    fn unique_completion_inserts_the_remainder() {
        let mut editor = Editor::new();
        editor.insert("car");
        let candidates = vec!["cargo".to_owned(), "cat".to_owned()];
        assert_eq!(editor.complete(&candidates), Completion::Inserted);
        assert_eq!(editor.text(), "cargo");
    }

    #[test]
    fn ambiguous_completion_inserts_the_common_prefix() {
        let mut editor = Editor::new();
        editor.insert("ca");
        let candidates = vec!["cargo".to_owned(), "cargo-test".to_owned()];
        assert_eq!(editor.complete(&candidates), Completion::Inserted);
        assert_eq!(editor.text(), "cargo");
        assert_eq!(editor.complete(&candidates), Completion::Ambiguous(vec![
            "cargo".to_owned(),
            "cargo-test".to_owned()
        ]));
    }

    #[test]
    fn take_clears_the_buffer() {
        let mut editor = Editor::new();
        editor.insert("ls -la");
        assert_eq!(editor.take(), "ls -la");
        assert!(editor.is_empty());
        assert_eq!(editor.cursor(), 0);
    }

    #[test]
    fn line_column_counts_characters_not_bytes() {
        let mut editor = Editor::new();
        editor.insert("áé");
        assert_eq!(editor.line_column(), (0, 2));
    }
}
