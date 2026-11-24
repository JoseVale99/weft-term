//! Fuzzy matching shared by the history search and the command palette.
//!
//! Scoring is deliberately simple and deterministic: the best match is the one
//! that matches the most characters consecutively and at word boundaries. There
//! is no dynamic programming, because a palette must answer in microseconds for
//! a few hundred entries.

/// A successful match: its score and the matched character positions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Match {
    /// Higher is better.
    pub score: u32,
    /// Character positions in the candidate, in increasing order.
    pub positions: Vec<usize>,
}

/// Matches `pattern` against `candidate` as a subsequence.
///
/// An empty pattern matches everything with score zero. Matching is
/// case-insensitive, but an exact case match scores slightly higher.
pub fn matches(pattern: &str, candidate: &str) -> Option<Match> {
    if pattern.is_empty() {
        return Some(Match {
            score: 0,
            positions: Vec::new(),
        });
    }

    let candidate: Vec<char> = candidate.chars().collect();
    let lowered: Vec<char> = candidate.iter().flat_map(|c| c.to_lowercase()).collect();
    // Lowercasing can change the length for some scripts; fall back to the
    // original characters so indexes always belong to `candidate`.
    let lowered: Vec<char> = if lowered.len() == candidate.len() {
        lowered
    } else {
        candidate.iter().map(|c| c.to_ascii_lowercase()).collect()
    };

    let mut positions = Vec::with_capacity(pattern.chars().count());
    let mut score = 0u32;
    let mut cursor = 0usize;

    for wanted in pattern.chars() {
        let wanted = wanted.to_ascii_lowercase();
        // Among every occurrence after the cursor, prefer the one that starts a
        // word, then the one next to the previous match, then the earliest.
        let mut best: Option<(u32, usize)> = None;
        for (offset, found_char) in lowered[cursor..].iter().enumerate() {
            if *found_char != wanted {
                continue;
            }
            let found = cursor + offset;
            let mut bonus = 0u32;
            if let Some(last) = positions.last()
                && *last + 1 == found
            {
                bonus += 8;
            }
            if found == 0 || is_boundary(candidate[found - 1]) {
                bonus += 12;
            }
            if best.is_none_or(|(best_bonus, _)| bonus > best_bonus) {
                best = Some((bonus, found));
            }
        }
        let (mut bonus, found) = best?;
        if candidate[found].is_uppercase() {
            bonus += 2;
        }
        score += 16 + bonus;
        positions.push(found);
        cursor = found + 1;
    }

    // Slight preference for matches that begin early.
    score = score.saturating_sub(positions.first().copied().unwrap_or(0).min(8) as u32);
    Some(Match { score, positions })
}

/// Ranks the candidates that match `pattern`, best first.
///
/// Ties keep the original order, which makes the output stable for tests and
/// for a palette that wants to preserve recency.
pub fn rank<'a>(
    pattern: &str,
    candidates: impl IntoIterator<Item = &'a str>,
) -> Vec<(&'a str, Match)> {
    let mut scored: Vec<(&'a str, Match)> = candidates
        .into_iter()
        .filter_map(|candidate| matches(pattern, candidate).map(|found| (candidate, found)))
        .collect();
    scored.sort_by(|a, b| b.1.score.cmp(&a.1.score));
    scored
}

fn is_boundary(c: char) -> bool {
    matches!(c, '/' | '\\' | '-' | '_' | '.' | ' ' | ':' | '@')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_pattern_matches_everything() {
        let matched = matches("", "cargo test").unwrap();
        assert_eq!(matched.score, 0);
        assert!(matched.positions.is_empty());
    }

    #[test]
    fn a_subsequence_matches_in_order() {
        assert!(matches("ct", "cargo test").is_some());
        assert!(matches("tc", "cargo test").is_none());
    }

    #[test]
    fn matching_is_case_insensitive() {
        assert!(matches("CARGO", "cargo test").is_some());
        assert!(matches("cargo", "CARGO TEST").is_some());
    }

    #[test]
    fn word_boundaries_score_higher_than_mid_word_matches() {
        let boundary = matches("st", "git status").unwrap();
        let inside = matches("st", "mystery").unwrap();
        assert!(boundary.score > inside.score);
    }

    #[test]
    fn exact_prefix_beats_a_later_match() {
        let early = matches("car", "cargo build").unwrap();
        let late = matches("car", "scaffold arc").unwrap();
        assert!(early.score > late.score);
    }

    #[test]
    fn ranking_is_stable_and_ordered() {
        let ranked = rank("st", ["git status", "cargo test", "ls"]);
        assert_eq!(ranked.len(), 2);
        assert!(ranked[0].1.score >= ranked[1].1.score);
    }

    #[test]
    fn positions_point_at_matched_characters() {
        let matched = matches("ct", "cargo test").unwrap();
        let chars: Vec<char> = "cargo test".chars().collect();
        assert_eq!(chars[matched.positions[0]], 'c');
        assert_eq!(chars[matched.positions[1]], 't');
    }
}
