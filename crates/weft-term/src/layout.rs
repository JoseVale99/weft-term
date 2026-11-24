//! Pane layout: spans, split tree and rectangle computation.
//!
//! The tree is pure data, so nesting, focus changes and closing panes can be
//! tested without a terminal. `weft` renders the rectangles and separators that
//! this module produces.

/// Direction in which an area is split.
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

/// Sizes only, in order.
pub fn heights(total: usize, count: usize) -> Vec<usize> {
    spans(total, count)
        .into_iter()
        .map(|span| span.size)
        .collect()
}

/// A rectangle in screen cells, zero-based.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rect {
    /// First column.
    pub x: usize,
    /// First row.
    pub y: usize,
    /// Width in cells.
    pub width: usize,
    /// Height in cells.
    pub height: usize,
}

/// A separator line between two panes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Segment {
    /// Orientation of the split that produced it.
    pub orientation: Orientation,
    /// First column.
    pub x: usize,
    /// First row.
    pub y: usize,
    /// Cells covered, along the perpendicular axis.
    pub length: usize,
}

/// One node of the split tree.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Node {
    /// A pane, identified by its stable id.
    Leaf(usize),
    /// Two children; `first` is on top or on the left.
    Split {
        /// Direction of this split.
        orientation: Orientation,
        /// Percentage of the usable cells given to `first`, between 10 and 90.
        ratio: u8,
        /// First child.
        first: Box<Node>,
        /// Second child.
        second: Box<Node>,
    },
}

impl Node {
    fn first_leaf(&self) -> Option<usize> {
        match self {
            Self::Leaf(id) => Some(*id),
            Self::Split { first, .. } => first.first_leaf(),
        }
    }

    fn contains(&self, id: usize) -> bool {
        match self {
            Self::Leaf(found) => *found == id,
            Self::Split { first, second, .. } => first.contains(id) || second.contains(id),
        }
    }

    fn split(&mut self, focus: usize, id: usize, orientation: Orientation) -> bool {
        match self {
            Self::Leaf(found) if *found == focus => {
                let previous = Self::Leaf(*found);
                *self = Self::Split {
                    orientation,
                    ratio: 50,
                    first: Box::new(previous),
                    second: Box::new(Self::Leaf(id)),
                };
                true
            }
            Self::Leaf(_) => false,
            Self::Split { first, second, .. } => {
                first.split(focus, id, orientation) || second.split(focus, id, orientation)
            }
        }
    }

    fn resize(&mut self, focus: usize, delta: i16) -> bool {
        let Self::Split {
            ratio,
            first,
            second,
            ..
        } = self
        else {
            return false;
        };
        let in_first = first.contains(focus);
        let in_second = second.contains(focus);
        if !in_first && !in_second {
            return false;
        }
        if first.resize(focus, delta) || second.resize(focus, delta) {
            return true;
        }
        let step = if in_first { delta } else { -delta };
        let next = (i16::from(*ratio) + step).clamp(10, 90);
        let changed = u8::try_from(next).unwrap_or(*ratio) != *ratio;
        if changed {
            *ratio = u8::try_from(next).unwrap_or(*ratio);
        }
        changed
    }

    fn remove(&mut self, focus: usize) -> Option<usize> {
        let Self::Split { first, second, .. } = self else {
            return None;
        };
        if matches!(&**first, Self::Leaf(id) if *id == focus) {
            let sibling = std::mem::replace(&mut **second, Self::Leaf(usize::MAX));
            *self = sibling;
            return self.first_leaf();
        }
        if matches!(&**second, Self::Leaf(id) if *id == focus) {
            let sibling = std::mem::replace(&mut **first, Self::Leaf(usize::MAX));
            *self = sibling;
            return self.first_leaf();
        }
        first.remove(focus).or_else(|| second.remove(focus))
    }

    fn leaves(&self, out: &mut Vec<usize>) {
        match self {
            Self::Leaf(id) => out.push(*id),
            Self::Split { first, second, .. } => {
                first.leaves(out);
                second.leaves(out);
            }
        }
    }

    fn layout(&self, area: Rect, out: &mut Layout) {
        match self {
            Self::Leaf(id) => out.panes.push((*id, area)),
            Self::Split {
                orientation,
                ratio,
                first,
                second,
            } => {
                let (lead, trail, separator) = split_area(area, *orientation, *ratio);
                first.layout(lead, out);
                second.layout(trail, out);
                if let Some(separator) = separator {
                    out.separators.push(separator);
                }
            }
        }
    }
}

/// Panes and separators for one area, in draw order.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Layout {
    /// Pane id and its rectangle, depth first.
    pub panes: Vec<(usize, Rect)>,
    /// Separators between panes.
    pub separators: Vec<Segment>,
}

/// A binary tree of panes with one focused leaf.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SplitTree {
    root: Node,
}

impl SplitTree {
    /// Starts a tree with a single pane.
    pub fn new(id: usize) -> Self {
        Self {
            root: Node::Leaf(id),
        }
    }

    /// Pane ids, left to right and top to bottom.
    pub fn leaves(&self) -> Vec<usize> {
        let mut out = Vec::new();
        self.root.leaves(&mut out);
        out
    }

    /// Whether the tree holds this pane.
    pub fn contains(&self, id: usize) -> bool {
        self.root.contains(id)
    }

    /// Splits the focused pane and puts `id` on top of it or to its right.
    pub fn split(&mut self, focus: usize, id: usize, orientation: Orientation) -> bool {
        self.root.split(focus, id, orientation)
    }

    /// Removes a pane. Returns the new focus, or `None` when the last pane was
    /// removed and the tab should close.
    pub fn remove(&mut self, focus: usize) -> Option<usize> {
        if !self.root.contains(focus) {
            return self.root.first_leaf();
        }
        if matches!(&self.root, Node::Leaf(_)) {
            return None;
        }
        self.root.remove(focus).or_else(|| self.root.first_leaf())
    }

    /// Grows (positive delta) or shrinks the focused pane by percentage points,
    /// adjusting its nearest enclosing split and clamping the ratio.
    pub fn resize(&mut self, focus: usize, delta: i16) -> bool {
        self.root.resize(focus, delta)
    }

    /// Computes pane rectangles and separators for an area.
    pub fn layout(&self, area: Rect) -> Layout {
        let mut out = Layout::default();
        self.root.layout(area, &mut out);
        out
    }
}

fn split_area(area: Rect, orientation: Orientation, ratio: u8) -> (Rect, Rect, Option<Segment>) {
    let total = match orientation {
        Orientation::Stacked => area.height,
        Orientation::SideBySide => area.width,
    };
    let usable = total.saturating_sub(1);
    if usable < 2 {
        // Too small for a separator: halve without a gap.
        let half = total / 2;
        return (
            shrink(area, orientation, 0, half),
            shrink(area, orientation, half, total - half),
            None,
        );
    }
    let lead_size = (usable * usize::from(ratio) + 50) / 100;
    let lead_size = lead_size.clamp(1, usable - 1);
    let trail_size = usable - lead_size;
    let separator_position = match orientation {
        Orientation::Stacked => area.y + lead_size,
        Orientation::SideBySide => area.x + lead_size,
    };
    let separator = match orientation {
        Orientation::Stacked => Segment {
            orientation,
            x: area.x,
            y: separator_position,
            length: area.width,
        },
        Orientation::SideBySide => Segment {
            orientation,
            x: separator_position,
            y: area.y,
            length: area.height,
        },
    };
    (
        shrink(area, orientation, 0, lead_size),
        shrink(area, orientation, lead_size + 1, trail_size),
        Some(separator),
    )
}

fn shrink(area: Rect, orientation: Orientation, start: usize, size: usize) -> Rect {
    match orientation {
        Orientation::Stacked => Rect {
            x: area.x,
            y: area.y + start,
            width: area.width,
            height: size,
        },
        Orientation::SideBySide => Rect {
            x: area.x + start,
            y: area.y,
            width: size,
            height: area.height,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn area() -> Rect {
        Rect {
            x: 0,
            y: 0,
            width: 80,
            height: 20,
        }
    }

    #[test]
    fn a_single_pane_gets_every_row() {
        assert_eq!(heights(24, 1), [24]);
        let slots = spans(24, 1);
        assert_eq!(slots.len(), 1);
        assert_eq!((slots[0].start, slots[0].size), (0, 24));
    }

    #[test]
    fn two_panes_share_the_rows_and_one_separator() {
        assert_eq!(heights(24, 2), [12, 11]);
        let slots = spans(24, 2);
        assert_eq!(slots.len(), 2);
        assert_eq!((slots[0].start, slots[0].size), (0, 12));
        assert_eq!((slots[1].start, slots[1].size), (13, 11));
        assert_eq!(heights(24, 2).iter().sum::<usize>() + 1, 24);
    }

    #[test]
    fn the_same_arithmetic_places_columns() {
        let slots = spans(80, 2);
        assert_eq!(slots.len(), 2);
        assert_eq!((slots[0].start, slots[0].size), (0, 40));
        assert_eq!((slots[1].start, slots[1].size), (41, 39));
    }

    #[test]
    fn impossible_layouts_are_rejected() {
        assert!(heights(1, 2).is_empty());
        assert!(heights(10, 0).is_empty());
        assert!(heights(2, 2).is_empty());
        assert!(spans(4, 3).is_empty());
    }

    #[test]
    fn a_single_leaf_fills_the_area() {
        let tree = SplitTree::new(7);
        let layout = tree.layout(area());
        assert_eq!(layout.panes.len(), 1);
        assert_eq!(layout.panes[0].0, 7);
        let rect = layout.panes[0].1;
        assert_eq!((rect.x, rect.y), (0, 0));
        assert_eq!((rect.width, rect.height), (80, 20));
        assert!(layout.separators.is_empty());
    }

    #[test]
    fn nested_splits_produce_mixed_rectangles() {
        let mut tree = SplitTree::new(1);
        assert!(tree.split(1, 2, Orientation::Stacked));
        assert!(tree.split(2, 3, Orientation::SideBySide));
        assert_eq!(tree.leaves(), [1, 2, 3]);

        let layout = tree.layout(area());
        assert_eq!(layout.panes.len(), 3);
        assert_eq!(layout.panes[0].0, 1);
        assert_eq!(layout.panes[1].0, 2);
        assert_eq!(layout.panes[2].0, 3);

        let top = layout.panes[0].1;
        assert_eq!((top.x, top.y), (0, 0));
        assert_eq!((top.width, top.height), (80, 10));

        let bottom_left = layout.panes[1].1;
        assert_eq!((bottom_left.x, bottom_left.y), (0, 11));
        assert_eq!((bottom_left.width, bottom_left.height), (40, 9));

        let bottom_right = layout.panes[2].1;
        assert_eq!((bottom_right.x, bottom_right.y), (41, 11));
        assert_eq!((bottom_right.width, bottom_right.height), (39, 9));

        assert_eq!(layout.separators.len(), 2);
        let vertical = layout.separators[0];
        assert_eq!(vertical.orientation, Orientation::SideBySide);
        assert_eq!((vertical.x, vertical.y), (40, 11));
        assert_eq!(vertical.length, 9);
        let horizontal = layout.separators[1];
        assert_eq!(horizontal.orientation, Orientation::Stacked);
        assert_eq!((horizontal.x, horizontal.y), (0, 10));
        assert_eq!(horizontal.length, 80);
    }

    #[test]
    fn closing_a_pane_promotes_its_sibling() {
        let mut tree = SplitTree::new(1);
        tree.split(1, 2, Orientation::Stacked);
        tree.split(2, 3, Orientation::SideBySide);

        assert_eq!(tree.remove(2), Some(3));
        assert_eq!(tree.leaves(), [1, 3]);
        assert_eq!(tree.remove(3), Some(1));
        assert_eq!(tree.leaves(), [1]);
        assert_eq!(tree.remove(1), None);
    }

    #[test]
    fn splitting_an_unknown_pane_changes_nothing() {
        let mut tree = SplitTree::new(1);
        assert!(!tree.split(99, 2, Orientation::Stacked));
        assert_eq!(tree.leaves(), [1]);
    }

    #[test]
    fn orientation_toggles() {
        assert_eq!(Orientation::default(), Orientation::Stacked);
        assert_eq!(Orientation::Stacked.toggled(), Orientation::SideBySide);
        assert_eq!(Orientation::SideBySide.toggled(), Orientation::Stacked);
        assert_eq!(Orientation::SideBySide.label(), "right");
    }

    #[test]
    fn resizing_moves_the_nearest_separator() {
        let mut tree = SplitTree::new(1);
        tree.split(1, 2, Orientation::Stacked);
        let before = tree.layout(area());
        assert_eq!(before.panes[0].1.height, 10);

        assert!(tree.resize(1, 10));
        let grown = tree.layout(area());
        assert_eq!(grown.panes[0].1.height, 11);
        assert_eq!(grown.panes[1].1.height, 8);
        assert_eq!(grown.separators[0].y, 11);

        assert!(tree.resize(2, 10));
        let shrunk = tree.layout(area());
        assert_eq!(shrunk.panes[0].1.height, 10);
        assert_eq!(shrunk.panes[1].1.height, 9);
    }

    #[test]
    fn resizing_clamps_and_ignores_unknown_panes() {
        let mut tree = SplitTree::new(1);
        tree.split(1, 2, Orientation::SideBySide);
        for _ in 0..20 {
            tree.resize(1, 10);
        }
        assert_eq!(tree.layout(area()).panes[0].1.width, 71);
        assert!(!tree.resize(99, 10));
    }

    #[test]
    fn nested_resize_uses_the_closest_split() {
        let mut tree = SplitTree::new(1);
        tree.split(1, 2, Orientation::Stacked);
        tree.split(2, 3, Orientation::SideBySide);
        assert!(tree.resize(3, 10));
        let layout = tree.layout(area());
        assert_eq!(layout.panes[1].1.width, 32);
        assert_eq!(layout.panes[2].1.width, 47);
        assert_eq!(layout.separators[0].x, 32);
    }
}
