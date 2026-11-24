//! Command history for the block shell and the editor.
//!
//! The history is a bounded, deduplicated list ordered oldest first. Navigating
//! it is a cursor over the list plus the line the user was typing, so moving
//! away and back never loses a draft.

use std::collections::VecDeque;

use crate::fuzzy;
use crate::input::Editor;

/// A bounded command history with fuzzy search.
#[derive(Debug, Clone)]
pub struct History {
    entries: VecDeque<String>,
    limit: usize,
    position: Option<usize>,
    draft: String,
}

impl History {
    /// Creates a history that keeps at most `limit` entries.
    ///
    /// A limit of zero is clamped to one: a history that stores nothing is a
    /// configuration error, not a useful state.
    pub fn new(limit: usize) -> Self {
        Self {
            entries: VecDeque::new(),
            limit: limit.max(1),
            position: None,
            draft: String::new(),
        }
    }

    /// Records a command, ignoring empty lines and immediate repeats.
    pub fn record(&mut self, line: &str) {
        let line = line.trim();
        if line.is_empty() || self.entries.back().is_some_and(|last| last == line) {
            return;
        }
        if self.entries.len() == self.limit {
            self.entries.pop_front();
        }
        self.entries.push_back(line.to_owned());
    }

    /// Returns the previous entry, or the same one when already at the oldest.
    ///
    /// The first call saves `draft` so [`History::next`] can restore it.
    pub fn previous(&mut self, draft: &str) -> Option<&str> {
        if self.entries.is_empty() {
            return None;
        }
        let position = match self.position {
            None => {
                self.draft = draft.to_owned();
                self.entries.len() - 1
            }
            Some(position) => position.saturating_sub(1),
        };
        self.position = Some(position);
        self.entries.get(position).map(String::as_str)
    }

    /// Returns the next entry, restoring the draft after the newest one.
    pub fn next_entry(&mut self) -> Option<&str> {
        let position = self.position?;
        if position + 1 >= self.entries.len() {
            self.position = None;
            return Some(self.draft.as_str());
        }
        self.position = Some(position + 1);
        self.entries.get(position + 1).map(String::as_str)
    }

    /// Forgets the navigation cursor. Call it after submitting or editing.
    pub fn reset(&mut self) {
        self.position = None;
        self.draft.clear();
    }

    /// Searches the history newest first with the fuzzy matcher.
    ///
    /// Duplicates are removed so a repeated command does not fill the list.
    pub fn search(&self, pattern: &str) -> Vec<&str> {
        let mut seen: Vec<&str> = Vec::new();
        let mut matched: Vec<(&str, u32, usize)> = Vec::new();
        for (recency, entry) in self.entries.iter().rev().enumerate() {
            if seen.contains(&entry.as_str()) {
                continue;
            }
            if let Some(found) = fuzzy::matches(pattern, entry) {
                seen.push(entry);
                matched.push((entry, found.score, recency));
            }
        }
        matched.sort_by(|a, b| b.1.cmp(&a.1).then(a.2.cmp(&b.2)));
        matched.into_iter().map(|(entry, _, _)| entry).collect()
    }

    /// Number of stored entries.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the history is empty.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// All entries, oldest first.
    pub fn entries(&self) -> impl DoubleEndedIterator<Item = &str> {
        self.entries.iter().map(String::as_str)
    }
}

/// Incremental reverse history search.
///
/// The editor shows a live preview: every query change selects the newest
/// fuzzy match, and accepting keeps it. Cancelling restores the line the user
/// was typing before the search started.
#[derive(Debug)]
pub struct Search {
    query: String,
    original: String,
    matches: Vec<String>,
    index: usize,
}

impl Search {
    /// Starts a search and remembers the editor's current text.
    pub fn start(editor: &Editor) -> Self {
        Self {
            query: String::new(),
            original: editor.text().to_owned(),
            matches: Vec::new(),
            index: 0,
        }
    }

    /// Current query.
    pub fn query(&self) -> &str {
        &self.query
    }

    /// Matches for the current query, newest first.
    pub fn matches(&self) -> &[String] {
        &self.matches
    }

    /// Appends a character, recomputes matches and previews the newest one.
    pub fn push(&mut self, c: char, history: &History, editor: &mut Editor) {
        self.query.push(c);
        self.refresh(history, editor);
    }

    /// Removes the last query character and recomputes.
    pub fn backspace(&mut self, history: &History, editor: &mut Editor) {
        self.query.pop();
        self.refresh(history, editor);
    }

    /// Recomputes the matches for the current query.
    pub fn refresh(&mut self, history: &History, editor: &mut Editor) {
        self.matches = history
            .search(&self.query)
            .into_iter()
            .map(str::to_owned)
            .collect();
        self.index = 0;
        self.preview(editor);
    }

    /// Moves to the next match, wrapping around.
    pub fn next(&mut self, editor: &mut Editor) {
        if self.matches.is_empty() {
            return;
        }
        self.index = (self.index + 1) % self.matches.len();
        self.preview(editor);
    }

    /// Restores the line that was being edited before the search.
    pub fn cancel(self, editor: &mut Editor) {
        editor.set_text(self.original);
    }

    fn preview(&self, editor: &mut Editor) {
        match self.matches.get(self.index) {
            Some(found) => editor.set_text(found.clone()),
            None => editor.set_text(self.original.clone()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_lines_and_immediate_repeats_are_ignored() {
        let mut history = History::new(10);
        history.record("ls");
        history.record("  ");
        history.record("ls");
        history.record("git status");
        assert_eq!(history.len(), 2);
    }

    #[test]
    fn the_limit_evicts_the_oldest_entry() {
        let mut history = History::new(2);
        history.record("one");
        history.record("two");
        history.record("three");
        assert_eq!(history.entries().collect::<Vec<_>>(), ["two", "three"]);
    }

    #[test]
    fn navigating_forward_restores_the_draft() {
        let mut history = History::new(10);
        history.record("first");
        history.record("second");

        assert_eq!(history.previous("dr"), Some("second"));
        assert_eq!(history.previous("dr"), Some("first"));
        assert_eq!(history.previous("dr"), Some("first"));
        assert_eq!(history.next_entry(), Some("second"));
        assert_eq!(history.next_entry(), Some("dr"));
        assert_eq!(history.next_entry(), None);
    }

    #[test]
    fn reset_ends_navigation() {
        let mut history = History::new(10);
        history.record("ls");
        assert_eq!(history.previous(""), Some("ls"));
        history.reset();
        assert_eq!(history.next_entry(), None);
    }

    #[test]
    fn search_is_fuzzy_and_newest_first() {
        let mut history = History::new(10);
        history.record("cargo build");
        history.record("git status");
        history.record("cargo test");

        let found = history.search("ct");
        assert_eq!(found.first(), Some(&"cargo test"));

        let all = history.search("");
        assert_eq!(all.len(), 3);
        assert_eq!(all[0], "cargo test");
    }

    #[test]
    fn search_does_not_repeat_entries() {
        let mut history = History::new(10);
        history.record("ls");
        history.record("pwd");
        history.record("ls");
        let found = history.search("ls");
        assert_eq!(found, ["ls"]);
    }

    #[test]
    fn search_previews_newest_match_and_cancels() {
        let mut history = History::new(10);
        history.record("cargo build");
        history.record("git status");
        history.record("cargo test");

        let mut editor = Editor::new();
        editor.set_text("draft");
        let mut search = Search::start(&editor);
        search.push('c', &history, &mut editor);
        search.push('a', &history, &mut editor);
        assert_eq!(search.query(), "ca");
        assert_eq!(editor.text(), "cargo test");
        assert_eq!(search.matches().len(), 2);

        search.next(&mut editor);
        assert_eq!(editor.text(), "cargo build");

        search.cancel(&mut editor);
        assert_eq!(editor.text(), "draft");
    }

    #[test]
    fn an_empty_query_keeps_the_original_when_there_is_no_history() {
        let history = History::new(10);
        let mut editor = Editor::new();
        editor.set_text("draft");
        let mut search = Search::start(&editor);
        search.refresh(&history, &mut editor);
        assert_eq!(editor.text(), "draft");
        assert!(search.matches().is_empty());
    }
}
