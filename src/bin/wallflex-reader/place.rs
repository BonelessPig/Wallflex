//! Where the reader's window goes: as a floating panel in the corner of
//! the monitor nearest the book icon, or docked full height against the
//! nearer side. All in desktop pixels.
//!
//! Sizes are given for a 1080p monitor at 100% scaling. Like Wallflex's
//! widgets, the window keeps its share of the monitor's height on
//! bigger monitors, and grows with Windows' scaling if that's more.

use serde::{Deserialize, Serialize};

/// The panel's size on a 1080p monitor at 100% scaling.
pub const PANEL_SIZE: (i32, i32) = (520, 720);
/// The docked pane's width on a 1080p monitor at 100% scaling.
pub const PANE_WIDTH: i32 = 520;
/// The floating panel's gap from the work area's edges, likewise.
pub const MARGIN: i32 = 12;
/// The smallest the window can be resized to, likewise.
pub const MIN_SIZE: (i32, i32) = (300, 300);
/// How wide the strip to grab for resizing is, likewise.
pub const RESIZE_BORDER: i32 = 2;
/// How far from a corner grabbing the strip resizes both ways, as a
/// multiple of the strip's width.
const CORNER_REACH: i32 = 8;
/// DPI at 100% scaling.
const BASE_DPI: i32 = 96;
/// The monitor height sizes are given for.
const BASE_HEIGHT: i32 = 1080;

/// How much bigger than given the window is drawn: a fraction, so
/// scaling stays in whole numbers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Scale {
    /// Numerator.
    num: i32,
    /// Denominator (never 0).
    den: i32,
}

impl Scale {
    /// The scale for a monitor `height` pixels tall at `dpi`: the larger
    /// of its scaling and its height against 1080p.
    pub fn new(dpi: u32, height: i32) -> Self {
        let by_dpi = Self {
            num: i32::try_from(dpi)
                .ok()
                .filter(|dpi| *dpi > 0)
                .unwrap_or(BASE_DPI),
            den: BASE_DPI,
        };
        let by_height = Self {
            num: height.max(1),
            den: BASE_HEIGHT,
        };
        let taller = i64::from(by_height.num)
            .saturating_mul(i64::from(BASE_DPI))
            > i64::from(by_dpi.num).saturating_mul(i64::from(BASE_HEIGHT));
        if taller { by_height } else { by_dpi }
    }

    /// `pixels` scaled (at least 1).
    pub fn apply(self, pixels: i32) -> i32 {
        let scaled = i64::from(pixels)
            .saturating_mul(i64::from(self.num))
            .checked_div(i64::from(self.den))
            .unwrap_or_default();
        i32::try_from(scaled).unwrap_or(i32::MAX).max(1)
    }

    /// `pixels` unscaled: the size given that comes out as `pixels`.
    pub fn unapply(self, pixels: i32) -> i32 {
        let unscaled = i64::from(pixels)
            .saturating_mul(i64::from(self.den))
            .checked_div(i64::from(self.num))
            .unwrap_or_default();
        i32::try_from(unscaled).unwrap_or(i32::MAX).max(1)
    }

    /// How much to zoom the page on a monitor at `dpi`, which already
    /// draws it at its scaling: the rest of this scale.
    pub fn zoom(self, dpi: u32) -> f64 {
        let dpi = f64::from(dpi.max(1));
        f64::from(self.num) * f64::from(BASE_DPI) / (f64::from(self.den) * dpi)
    }
}

/// A rectangle in desktop pixels; right and bottom are exclusive.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Rect {
    /// Left edge.
    pub left: i32,
    /// Top edge.
    pub top: i32,
    /// Right edge.
    pub right: i32,
    /// Bottom edge.
    pub bottom: i32,
}

impl Rect {
    /// Width (never negative).
    pub fn width(self) -> i32 {
        self.right.saturating_sub(self.left).max(0)
    }

    /// Height (never negative).
    pub fn height(self) -> i32 {
        self.bottom.saturating_sub(self.top).max(0)
    }

    /// The middle, rounded down.
    fn middle(self) -> (i32, i32) {
        (
            self.left.saturating_add(self.width() / 2),
            self.top.saturating_add(self.height() / 2),
        )
    }
}

/// The side of the monitor a docked pane sits against (`"left"` or
/// `"right"` in messages and `reader.json`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Edge {
    /// The left side.
    Left,
    /// The right side.
    Right,
}

/// Which of the window's sides can be dragged to resize it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Resizable {
    /// None (before it's first placed).
    Not,
    /// Every side and corner: the floating panel.
    All,
    /// Only this side: a docked pane's inner one.
    Side(Edge),
}

impl Resizable {
    /// The inner side of a pane docked against `edge`.
    pub fn inner(edge: Edge) -> Self {
        Self::Side(match edge {
            Edge::Left => Edge::Right,
            Edge::Right => Edge::Left,
        })
    }

    /// Whether the `edge` side can be dragged.
    fn across(self, edge: Edge) -> bool {
        match self {
            Self::Not => false,
            Self::All => true,
            Self::Side(side) => side == edge,
        }
    }

    /// Whether the top and bottom can be dragged.
    fn down(self) -> bool {
        self == Self::All
    }
}

/// The top or bottom of the window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Vertical {
    /// The top.
    Top,
    /// The bottom.
    Bottom,
}

/// What a resize grab moves: a side, a corner (one of each), or nothing.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Grab {
    /// The left or right side.
    pub across: Option<Edge>,
    /// The top or bottom.
    pub down: Option<Vertical>,
}

/// `window` less a `border` wide strip along each `resizable` side:
/// where the browser goes, leaving the strips to grab.
pub fn inset(window: Rect, resizable: Resizable, border: i32) -> Rect {
    let take = |side: bool| if side { border } else { 0 };
    let down = take(resizable.down());
    Rect {
        left: window
            .left
            .saturating_add(take(resizable.across(Edge::Left))),
        top: window.top.saturating_add(down),
        right: window
            .right
            .saturating_sub(take(resizable.across(Edge::Right))),
        bottom: window.bottom.saturating_sub(down),
    }
}

/// What a grab at `at` moves: a `resizable` side within `border` of it,
/// plus the neighbouring one when it's near a corner.
pub fn grab(
    window: Rect,
    resizable: Resizable,
    border: i32,
    at: (i32, i32),
) -> Grab {
    let near =
        |from: i32, to: i32, reach: i32| from.saturating_sub(to).abs() < reach;
    let across = |reach: i32| {
        if resizable.across(Edge::Left) && near(at.0, window.left, reach) {
            Some(Edge::Left)
        } else if resizable.across(Edge::Right)
            && near(at.0, window.right, reach)
        {
            Some(Edge::Right)
        } else {
            None
        }
    };
    let down = |reach: i32| {
        if !resizable.down() {
            None
        } else if near(at.1, window.top, reach) {
            Some(Vertical::Top)
        } else if near(at.1, window.bottom, reach) {
            Some(Vertical::Bottom)
        } else {
            None
        }
    };
    let corner = border.saturating_mul(CORNER_REACH);
    match (across(border), down(border)) {
        (Some(side), None) => Grab {
            across: Some(side),
            down: down(corner),
        },
        (None, Some(end)) => Grab {
            across: across(corner),
            down: Some(end),
        },
        (across, down) => Grab { across, down },
    }
}

/// The floating panel, `size` big, in the corner of `work` (the work
/// area) nearest `at`, `margin` in from its edges. Shrinks to fit.
pub fn floating(
    work: Rect,
    at: (i32, i32),
    size: (i32, i32),
    margin: i32,
) -> Rect {
    let room =
        |length: i32| length.saturating_sub(margin.saturating_mul(2)).max(1);
    let width = size.0.min(room(work.width()));
    let height = size.1.min(room(work.height()));
    let (middle_x, middle_y) = work.middle();
    let left = if at.0 >= middle_x {
        work.right.saturating_sub(margin).saturating_sub(width)
    } else {
        work.left.saturating_add(margin)
    };
    let top = if at.1 >= middle_y {
        work.bottom.saturating_sub(margin).saturating_sub(height)
    } else {
        work.top.saturating_add(margin)
    };
    Rect {
        left,
        top,
        right: left.saturating_add(width),
        bottom: top.saturating_add(height),
    }
}

/// The side of `monitor` nearer `at`.
pub fn edge_for(monitor: Rect, at: (i32, i32)) -> Edge {
    if at.0 >= monitor.middle().0 {
        Edge::Right
    } else {
        Edge::Left
    }
}

/// The docked pane: `width` wide (at most half of `work`) and the full
/// height of `work`, against `edge`.
pub fn docked(work: Rect, edge: Edge, width: i32) -> Rect {
    let width = width.min(work.width() / 2).max(1);
    match edge {
        Edge::Left => Rect {
            right: work.left.saturating_add(width),
            ..work
        },
        Edge::Right => Rect {
            left: work.right.saturating_sub(width),
            ..work
        },
    }
}

#[cfg(test)]
mod tests {
    use super::{
        Edge, Grab, Rect, Resizable, Scale, Vertical, docked, edge_for,
        floating, grab, inset,
    };

    /// A 1920x1080 monitor's work area with a 48 px taskbar.
    const WORK: Rect = Rect {
        left: 0,
        top: 0,
        right: 1920,
        bottom: 1032,
    };

    #[test]
    fn scales_with_dpi_or_height() {
        let at_150 = Scale::new(144, 1080);
        assert_eq!(at_150.apply(520), 780);
        assert!((at_150.zoom(144) - 1.0).abs() < 1e-9);
        let uhd_at_100 = Scale::new(96, 2160);
        assert_eq!(uhd_at_100.apply(520), 1040);
        assert!((uhd_at_100.zoom(96) - 2.0).abs() < 1e-9);
        let uhd_at_250 = Scale::new(240, 2160);
        assert_eq!(uhd_at_250.apply(520), 1300);
        assert_eq!(Scale::new(0, 0).apply(12), 12);
        assert_eq!(at_150.unapply(780), 520);
        assert_eq!(uhd_at_100.unapply(1040), 520);
    }

    #[test]
    fn floats_in_the_nearest_corner() {
        let size = (520, 720);
        let bottom_right = floating(WORK, (1900, 1000), size, 12);
        assert_eq!(
            bottom_right,
            Rect {
                left: 1388,
                top: 300,
                right: 1908,
                bottom: 1020
            }
        );
        let top_left = floating(WORK, (10, 10), size, 12);
        assert_eq!((top_left.left, top_left.top), (12, 12));
    }

    #[test]
    fn floating_shrinks_to_fit() {
        let small = Rect {
            left: -800,
            top: 0,
            right: 0,
            bottom: 600,
        };
        let panel = floating(small, (-10, 590), (520, 720), 12);
        assert_eq!(panel.height(), 576);
        assert_eq!(panel.top, 12);
        assert_eq!(panel.right, -12);
    }

    #[test]
    fn docks_against_the_nearer_side() {
        assert_eq!(edge_for(WORK, (1500, 10)), Edge::Right);
        assert_eq!(edge_for(WORK, (100, 10)), Edge::Left);
        let pane = docked(WORK, Edge::Right, 520);
        assert_eq!(pane, Rect { left: 1400, ..WORK });
        assert_eq!(docked(WORK, Edge::Left, 5_000).width(), 960);
    }

    #[test]
    fn insets_the_resizable_sides() {
        assert_eq!(
            inset(WORK, Resizable::All, 6),
            Rect {
                left: 6,
                top: 6,
                right: 1914,
                bottom: 1026
            }
        );
        let pane = inset(WORK, Resizable::inner(Edge::Right), 6);
        assert_eq!(pane, Rect { left: 6, ..WORK });
        assert_eq!(inset(WORK, Resizable::Not, 6), WORK);
    }

    #[test]
    fn grabs_sides_and_corners() {
        let all = Resizable::All;
        assert_eq!(grab(WORK, all, 6, (900, 500)), Grab::default());
        assert_eq!(
            grab(WORK, all, 6, (2, 500)),
            Grab {
                across: Some(Edge::Left),
                down: None
            }
        );
        // On the top strip, near the right corner: both ways.
        assert_eq!(
            grab(WORK, all, 6, (1910, 3)),
            Grab {
                across: Some(Edge::Right),
                down: Some(Vertical::Top)
            }
        );
        // A docked pane only gives its inner side.
        let inner = Resizable::inner(Edge::Left);
        assert_eq!(grab(WORK, inner, 6, (2, 500)), Grab::default());
        assert_eq!(
            grab(WORK, inner, 6, (1917, 2)),
            Grab {
                across: Some(Edge::Right),
                down: None
            }
        );
    }
}
