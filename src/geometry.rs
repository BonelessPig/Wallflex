//! Rectangles and points in physical pixels.

/// A point in physical pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Point {
    /// Horizontal position; grows to the right.
    pub x: i32,
    /// Vertical position; grows downwards.
    pub y: i32,
}

/// A rectangle in physical pixels. `left`/`top` are inside it,
/// `right`/`bottom` are just outside, as in Win32.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rect {
    /// Left edge (inclusive).
    pub left: i32,
    /// Top edge (inclusive).
    pub top: i32,
    /// Right edge (exclusive).
    pub right: i32,
    /// Bottom edge (exclusive).
    pub bottom: i32,
}

impl Rect {
    /// Width in pixels; 0 for an inverted rectangle.
    pub fn width(self) -> i32 {
        self.right.saturating_sub(self.left).max(0)
    }

    /// Height in pixels; 0 for an inverted rectangle.
    pub fn height(self) -> i32 {
        self.bottom.saturating_sub(self.top).max(0)
    }

    /// Whether the two rectangles share any pixels.
    pub fn intersects(self, other: Self) -> bool {
        self.left < other.right
            && other.left < self.right
            && self.top < other.bottom
            && other.top < self.bottom
    }

    /// The pixels both rectangles share, if any.
    pub fn intersection(self, other: Self) -> Option<Self> {
        self.intersects(other).then(|| Self {
            left: self.left.max(other.left),
            top: self.top.max(other.top),
            right: self.right.min(other.right),
            bottom: self.bottom.min(other.bottom),
        })
    }

    /// The same rectangle measured from `origin` instead of from the
    /// desktop's (0, 0). Used to turn a monitor's desktop position into a
    /// position inside a parent window whose top-left is at `origin`.
    pub fn relative_to(self, origin: Point) -> Self {
        Self {
            left: self.left.saturating_sub(origin.x),
            top: self.top.saturating_sub(origin.y),
            right: self.right.saturating_sub(origin.x),
            bottom: self.bottom.saturating_sub(origin.y),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Point, Rect};

    /// The upper 3440x1440 monitor from the plan, above the main one.
    const UPPER: Rect = Rect {
        left: 226,
        top: -1440,
        right: 3666,
        bottom: 0,
    };

    #[test]
    fn measures_size() {
        assert_eq!(UPPER.width(), 3440);
        assert_eq!(UPPER.height(), 1440);
    }

    #[test]
    fn inverted_rect_has_no_size() {
        let r = Rect {
            left: 10,
            top: 10,
            right: 0,
            bottom: 0,
        };
        assert_eq!((r.width(), r.height()), (0, 0));
    }

    #[test]
    fn detects_overlap() {
        let panel = Rect {
            left: 100,
            top: 100,
            right: 200,
            bottom: 200,
        };
        let touching = Rect {
            left: 200,
            top: 100,
            right: 300,
            bottom: 200,
        };
        let overlapping = Rect {
            left: 150,
            top: 150,
            right: 160,
            bottom: 160,
        };
        assert!(!panel.intersects(touching));
        assert!(panel.intersects(overlapping));
        assert!(overlapping.intersects(panel));
    }

    #[test]
    fn moves_into_parent_space() {
        // A parent covering the whole desktop starts at the top-left of
        // the upper monitor's row: (0, -1440).
        let origin = Point { x: 0, y: -1440 };
        let inside = UPPER.relative_to(origin);
        assert_eq!(
            inside,
            Rect {
                left: 226,
                top: 0,
                right: 3666,
                bottom: 1440,
            }
        );
    }
}
