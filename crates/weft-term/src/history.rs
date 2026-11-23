//! Command history for the block shell and the editor.
//!
//! The history is a bounded, deduplicated list ordered oldest first. Navigating
//! it is a cursor over the list plus the line the user was typing, so moving
//! away and back never loses a draft.

use std::collections::VecDeque;

use crate::fuzzy;

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
    pub fn next(&mut self) -> Option<&str> {
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
        assert_eq!(history.next(), Some("second"));
        assert_eq!(history.next(), Some("dr"));
        assert_eq!(history.next(), None);
    }

    #[test]
    fn reset_ends_navigation() {
        let mut history = History::new(10);
        history.record("ls");
        assert_eq!(history.previous(""), Some("ls"));
        history.reset();
        assert_eq!(history.next(), None);
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
}
