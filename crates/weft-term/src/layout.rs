//! Pane layout.
//!
//! Splits stack panes vertically or place them side by side, always reserving
//! one separator cell between neighbours. The arithmetic lives here, without a
//! renderer, so it can be tested directly.

/// Direction in which a tab is split.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Orientation {
    /// Panes stacked top to bottom.
    #[default]
    Stacked,
    /// Panes placed left to right.
    SideBySide,
}

impl Orientation {
    /// The other direction.
    pub fn toggled(self) -> Self {
        match self {
            Self::Stacked => Self::SideBySide,
            Self::SideBySide => Self::Stacked,
        }
    }

    /// Human readable name.
    pub fn label(self) -> &'static str {
        match self {
            Self::Stacked => "down",
            Self::SideBySide => "right",
        }
    }
}

/// Allocation for one pane along an axis: start offset and size.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Span {
    /// First cell of the pane, counting the separators before it.
    pub start: usize,
    /// Cells available to the pane.
    pub size: usize,
}

/// Splits `total` cells among `count` panes.
///
/// One cell is reserved between consecutive panes. Earlier panes receive the
/// remainder, so sizes differ by at most one cell. Returns an empty vector when
/// there is nothing to lay out or the room cannot give every pane at least one
/// cell plus the separators.
pub fn spans(total: usize, count: usize) -> Vec<Span> {
    if count == 0 || total < count + count.saturating_sub(1) {
        return Vec::new();
    }
    let available = total - count.saturating_sub(1);
    let base = available / count;
    let extra = available % count;
    let mut start = 0;
    (0..count)
        .map(|index| {
            let size = base + usize::from(index < extra);
            let span = Span { start, size };
            start += size + 1;
            span
        })
        .collect()
}

/// Sizes only, in order. Kept for callers that do not need the offsets.
pub fn heights(total: usize, count: usize) -> Vec<usize> {
    spans(total, count).into_iter().map(|span| span.size).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_single_pane_gets_every_row() {
        assert_eq!(heights(24, 1), [24]);
        assert_eq!(spans(24, 1), [Span { start: 0, size: 24 }]);
    }

    #[test]
    fn two_panes_share_the_rows_and_one_separator() {
        assert_eq!(heights(24, 2), [12, 11]);
        assert_eq!(
            spans(24, 2),
            [Span { start: 0, size: 12 }, Span { start: 13, size: 11 }]
        );
        assert_eq!(heights(24, 2).iter().sum::<usize>() + 1, 24);
    }

    #[test]
    fn three_panes_keep_the_order_and_total() {
        let rows = heights(10, 3);
        assert_eq!(rows, [3, 3, 2]);
        assert_eq!(rows.iter().sum::<usize>() + 2, 10);
        assert_eq!(
            spans(10, 3),
            [
                Span { start: 0, size: 3 },
                Span { start: 4, size: 3 },
                Span { start: 8, size: 2 },
            ]
        );
    }

    #[test]
    fn the_same_arithmetic_places_columns() {
        assert_eq!(
            spans(80, 2),
            [Span { start: 0, size: 40 }, Span { start: 41, size: 39 }]
        );
    }

    #[test]
    fn impossible_layouts_are_rejected() {
        assert!(heights(1, 2).is_empty());
        assert!(heights(10, 0).is_empty());
        assert!(heights(2, 2).is_empty());
        assert!(spans(4, 3).is_empty());
    }

    #[test]
    fn orientation_toggles() {
        assert_eq!(Orientation::default(), Orientation::Stacked);
        assert_eq!(Orientation::Stacked.toggled(), Orientation::SideBySide);
        assert_eq!(Orientation::SideBySide.toggled(), Orientation::Stacked);
        assert_eq!(Orientation::SideBySide.label(), "right");
    }
}
