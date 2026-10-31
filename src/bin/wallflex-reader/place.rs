//! Where the reader's window goes: as a floating panel in the corner of
//! the monitor nearest the book icon, or docked full height against the
//! nearer side. All in desktop pixels.
//!
//! Sizes are given for a 1080p monitor at 100% scaling. Like Wallflex's
//! widgets, the window keeps its share of the monitor's height on
//! bigger monitors, and grows with Windows' scaling if that's more.

/// The panel's size on a 1080p monitor at 100% scaling.
pub const PANEL_SIZE: (i32, i32) = (520, 720);
/// The docked pane's width on a 1080p monitor at 100% scaling.
pub const PANE_WIDTH: i32 = 520;
/// The floating panel's gap from the work area's edges, likewise.
pub const MARGIN: i32 = 12;
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

/// The side of the monitor a docked pane sits against.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Edge {
    /// The left side.
    Left,
    /// The right side.
    Right,
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
    use super::{Edge, Rect, Scale, docked, edge_for, floating};

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
}
