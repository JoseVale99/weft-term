//! Pane layout.
//!
//! Splits stack panes vertically and reserve one separator row between them.
//! The arithmetic lives here, without a renderer, so it can be tested directly.

/// Splits `total` rows among `count` panes.
///
/// One row is reserved between consecutive panes. Earlier panes receive the
/// remainder, so the widths differ by at most one row. Returns an empty vector
/// when there is nothing to lay out or the rows cannot give every pane at least
/// one row plus the separators.
pub fn heights(total: usize, count: usize) -> Vec<usize> {
    if count == 0 || total < count + count.saturating_sub(1) {
        return Vec::new();
    }
    let available = total - count.saturating_sub(1);
    let base = available / count;
    let extra = available % count;
    (0..count)
        .map(|index| base + usize::from(index < extra))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_single_pane_gets_every_row() {
        assert_eq!(heights(24, 1), [24]);
    }

    #[test]
    fn two_panes_share_the_rows_and_one_separator() {
        assert_eq!(heights(24, 2), [12, 11]);
        assert_eq!(heights(24, 2).iter().sum::<usize>() + 1, 24);
    }

    #[test]
    fn three_panes_keep_the_order_and_total() {
        let rows = heights(10, 3);
        assert_eq!(rows, [3, 3, 2]);
        assert_eq!(rows.iter().sum::<usize>() + 2, 10);
    }

    #[test]
    fn impossible_layouts_are_rejected() {
        assert!(heights(1, 2).is_empty());
        assert!(heights(10, 0).is_empty());
        assert!(heights(2, 2).is_empty());
    }
}
