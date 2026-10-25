//! What's behind the widgets on a monitor: a colour or nothing (so
//! the Windows wallpaper shows through), optionally with a picture on
//! top, and the maths that places the picture on each monitor.
//!
//! A picture is first fitted to a *canvas* in desktop pixels: the
//! monitor itself, or for `span` the box around every monitor sharing
//! the picture. Each monitor then shows its part of the canvas, zoomed
//! about an anchor and shifted by its own offset. For a span the
//! anchor is the middle of the edges it shares with the others, so
//! zooming one monitor keeps the picture lined up across the seam, and
//! the zoom defaults to what makes the picture physically the same
//! size on every monitor (from each monitor's size in millimetres).

use std::path::PathBuf;

use crate::color::Rgb;
use crate::geometry::{Point, Rect};

/// What fills the parts of a monitor no picture covers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Background {
    /// A solid colour.
    Color(Rgb),
    /// Nothing: the Windows wallpaper shows through.
    Transparent,
}

/// How a picture is fitted, like Windows' own wallpaper choices.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Fit {
    /// Scaled to cover the monitor, cropping what spills over.
    #[default]
    Fill,
    /// Scaled to fit inside the monitor, leaving bars of background
    /// ("fit" in the config).
    Contain,
    /// Stretched to the monitor's exact size.
    Stretch,
    /// At its own size in the middle, cropped if it's bigger.
    Center,
    /// One picture across every monitor using it, covering them all.
    Span,
}

/// A monitor's picture and how it's placed there.
#[derive(Clone, Debug, PartialEq)]
pub struct Wallpaper {
    /// The image file.
    pub path: PathBuf,
    /// How it's fitted.
    pub fit: Fit,
    /// How much bigger it's drawn on this monitor (1.0 = as fitted), if
    /// set; else automatic (see [`views`]).
    pub zoom: Option<f64>,
    /// How far it's moved on this monitor, in its pixels (right, down).
    pub offset: (f64, f64),
}

/// Everything behind the widgets on one monitor.
#[derive(Clone, Debug, PartialEq)]
pub struct Backdrop {
    /// The colour, or transparency.
    pub background: Background,
    /// A picture on top of it, if any.
    pub image: Option<Wallpaper>,
}

impl Backdrop {
    /// Whether the widgets need windows of their own, see-through
    /// around their shapes: true unless it's a plain colour, which the
    /// widgets can simply be drawn on.
    pub fn layered(&self) -> bool {
        self.image.is_some() || self.background == Background::Transparent
    }
}

/// A monitor, as far as placing pictures cares.
#[derive(Clone, Copy, Debug)]
pub struct Screen<'a> {
    /// Where it is, in desktop pixels.
    pub rect: Rect,
    /// Its picture, if any.
    pub image: Option<&'a Wallpaper>,
    /// Millimetres per pixel, if Windows knows its size.
    pub pitch: Option<f64>,
}

/// How one monitor shows its picture: the picture fitted to `canvas`,
/// then zoomed about `anchor` and shifted by `offset`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct View {
    /// How the picture is fitted to the canvas.
    pub fit: Fit,
    /// What the picture is fitted to, in desktop pixels.
    pub canvas: Rect,
    /// The point zoom keeps still, in desktop pixels.
    pub anchor: (f64, f64),
    /// How much bigger than fitted it's drawn here.
    pub zoom: f64,
    /// How far it's moved here, in this monitor's pixels.
    pub offset: (f64, f64),
}

/// Where a picture goes on one monitor: which part of it, at what
/// scale, lands where.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Mapping {
    /// The size to scale the whole picture to, in pixels.
    pub scaled: (u32, u32),
    /// The part of the scaled picture that shows (scaled pixels).
    pub source: Rect,
    /// Where that part goes on the monitor (monitor pixels, from its
    /// top-left). Always the same size as `source`.
    pub dest: Rect,
}

/// How each of `screens` shows its picture (`None` for those without
/// one). Monitors spanning the same picture share one canvas.
pub fn views(screens: &[Screen<'_>]) -> Vec<Option<View>> {
    screens.iter().map(|screen| view(screen, screens)).collect()
}

/// [`views`] for one screen.
fn view(screen: &Screen<'_>, screens: &[Screen<'_>]) -> Option<View> {
    let image = screen.image?;
    if image.fit != Fit::Span {
        return Some(View {
            fit: image.fit,
            canvas: screen.rect,
            anchor: centre(screen.rect),
            zoom: image.zoom.unwrap_or(1.0),
            offset: image.offset,
        });
    }
    let group: Vec<&Screen<'_>> = screens
        .iter()
        .filter(|other| {
            other
                .image
                .is_some_and(|w| w.fit == Fit::Span && w.path == image.path)
        })
        .collect();
    let canvas = group.iter().map(|s| s.rect).fold(screen.rect, Rect::union);
    let rects: Vec<Rect> = group.iter().map(|s| s.rect).collect();
    Some(View {
        fit: Fit::Span,
        canvas,
        anchor: shared_anchor(screen.rect, &rects),
        zoom: image.zoom.unwrap_or_else(|| auto_zoom(screen, &group)),
        offset: image.offset,
    })
}

/// The zoom that makes a spanned picture physically the same size on
/// `screen` as on the group's coarsest monitor (which gets 1.0). 1.0
/// when any monitor's size is unknown.
fn auto_zoom(screen: &Screen<'_>, group: &[&Screen<'_>]) -> f64 {
    let pitches: Option<Vec<f64>> = group
        .iter()
        .map(|s| s.pitch.filter(|p| p.is_finite() && *p > 0.0))
        .collect();
    let (Some(pitches), Some(own)) = (pitches, screen.pitch) else {
        return 1.0;
    };
    let coarsest = pitches.into_iter().fold(0.0, f64::max);
    if own > 0.0 && coarsest > 0.0 {
        coarsest / own
    } else {
        1.0
    }
}

/// The point a monitor at `rect` zooms about: the middle of the edges
/// it shares with `others` (weighted by length), or its centre if it
/// shares none.
pub fn shared_anchor(rect: Rect, others: &[Rect]) -> (f64, f64) {
    let mut total = 0.0;
    let (mut sum_x, mut sum_y) = (0.0, 0.0);
    for other in others.iter().filter(|o| **o != rect) {
        if let Some((mid, length)) = shared_edge(rect, *other) {
            sum_x += mid.0 * length;
            sum_y += mid.1 * length;
            total += length;
        }
    }
    if total > 0.0 {
        (sum_x / total, sum_y / total)
    } else {
        centre(rect)
    }
}

/// Where `a` and `b` touch: the middle of the shared edge and its
/// length, if they share one.
fn shared_edge(a: Rect, b: Rect) -> Option<((f64, f64), f64)> {
    let f = f64::from;
    // One above the other.
    let horizontal = if a.bottom == b.top {
        Some(a.bottom)
    } else if a.top == b.bottom {
        Some(a.top)
    } else {
        None
    };
    if let Some(y) = horizontal {
        let (from, to) = (a.left.max(b.left), a.right.min(b.right));
        if to > from {
            let length = f(to) - f(from);
            return Some(((f(from) + length / 2.0, f(y)), length));
        }
    }
    // Side by side.
    let vertical = if a.right == b.left {
        Some(a.right)
    } else if a.left == b.right {
        Some(a.left)
    } else {
        None
    };
    let x = vertical?;
    let (from, to) = (a.top.max(b.top), a.bottom.min(b.bottom));
    (to > from).then(|| {
        let length = f(to) - f(from);
        ((f(x), f(from) + length / 2.0), length)
    })
}

/// The middle of `rect`, in fractional pixels.
fn centre(rect: Rect) -> (f64, f64) {
    (
        f64::midpoint(f64::from(rect.left), f64::from(rect.right)),
        f64::midpoint(f64::from(rect.top), f64::from(rect.bottom)),
    )
}

/// Where an `image` sized picture (width, height in pixels) lands on
/// the monitor at `monitor` (desktop pixels) for `view`. `None` if the
/// picture or canvas is empty, or none of it reaches the monitor.
pub fn map(view: &View, image: (u32, u32), monitor: Rect) -> Option<Mapping> {
    let (image_w, image_h) = (f64::from(image.0), f64::from(image.1));
    let canvas = view.canvas;
    let (canvas_w, canvas_h) =
        (f64::from(canvas.width()), f64::from(canvas.height()));
    let usable = |v: f64| v.is_finite() && v > 0.0;
    if ![image_w, image_h, canvas_w, canvas_h, view.zoom]
        .into_iter()
        .all(usable)
    {
        return None;
    }
    // Canvas pixels per picture pixel, across and down.
    let (across, down) = (canvas_w / image_w, canvas_h / image_h);
    let (scale_x, scale_y) = match view.fit {
        Fit::Fill | Fit::Span => (across.max(down), across.max(down)),
        Fit::Contain => (across.min(down), across.min(down)),
        Fit::Stretch => (across, down),
        Fit::Center => (1.0, 1.0),
    };
    // The picture's top-left on the canvas: centred.
    let corner_x =
        f64::from(canvas.left) + (canvas_w - image_w * scale_x) / 2.0;
    let corner_y = f64::from(canvas.top) + (canvas_h - image_h * scale_y) / 2.0;
    // Zoomed about the anchor and shifted, relative to the monitor.
    let (anchor_x, anchor_y) = view.anchor;
    let left = anchor_x + view.zoom * (corner_x - anchor_x) + view.offset.0
        - f64::from(monitor.left);
    let top = anchor_y + view.zoom * (corner_y - anchor_y) + view.offset.1
        - f64::from(monitor.top);
    let scaled = (
        to_u32(image_w * scale_x * view.zoom)?,
        to_u32(image_h * scale_y * view.zoom)?,
    );
    let corner = Point {
        x: to_i32(left)?,
        y: to_i32(top)?,
    };
    let placed = Rect {
        left: corner.x,
        top: corner.y,
        right: corner.x.checked_add(i32::try_from(scaled.0).ok()?)?,
        bottom: corner.y.checked_add(i32::try_from(scaled.1).ok()?)?,
    };
    let screen = Rect {
        left: 0,
        top: 0,
        right: monitor.width(),
        bottom: monitor.height(),
    };
    let dest = placed.intersection(screen)?;
    Some(Mapping {
        scaled,
        source: dest.relative_to(corner),
        dest,
    })
}

/// `value` rounded to a whole pixel count, at least 1; `None` if it's
/// beyond what a picture can be scaled to.
fn to_u32(value: f64) -> Option<u32> {
    let rounded = value.round().max(1.0);
    (rounded <= f64::from(i32::MAX)).then(|| {
        #[expect(
            clippy::as_conversions,
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "checked to be 1 to i32::MAX and whole just above"
        )]
        let whole = rounded as u32;
        whole
    })
}

/// `value` rounded to a whole pixel position; `None` if it's beyond the
/// `i32` range.
fn to_i32(value: f64) -> Option<i32> {
    let rounded = value.round();
    let fits = rounded >= f64::from(i32::MIN) && rounded <= f64::from(i32::MAX);
    fits.then(|| {
        #[expect(
            clippy::as_conversions,
            clippy::cast_possible_truncation,
            reason = "checked to be whole and in the i32 range just above"
        )]
        let whole = rounded as i32;
        whole
    })
}

#[cfg(test)]
mod tests {
    use super::{
        Backdrop, Background, Fit, Mapping, Screen, View, Wallpaper, map,
        shared_anchor, views,
    };
    use crate::color::Rgb;
    use crate::geometry::Rect;

    const fn rect(left: i32, top: i32, right: i32, bottom: i32) -> Rect {
        Rect {
            left,
            top,
            right,
            bottom,
        }
    }

    /// The 4K monitor below, and the ultrawide above, as on the
    /// author's desk.
    const LOWER: Rect = rect(0, 0, 3840, 2160);
    const UPPER: Rect = rect(226, -1440, 3666, 0);

    /// A view of `monitor` alone, as fitted.
    fn alone(fit: Fit, monitor: Rect) -> View {
        let image = Wallpaper {
            path: "a.jpg".into(),
            fit,
            zoom: None,
            offset: (0.0, 0.0),
        };
        let screen = Screen {
            rect: monitor,
            image: Some(&image),
            pitch: None,
        };
        views(&[screen])[0].unwrap()
    }

    fn mapped(fit: Fit, image: (u32, u32), monitor: Rect) -> Mapping {
        map(&alone(fit, monitor), image, monitor).unwrap()
    }

    #[test]
    fn fill_covers_and_crops_the_middle() {
        let monitor = rect(0, 0, 3440, 1440);
        let m = mapped(Fit::Fill, (1920, 1080), monitor);
        assert_eq!(m.scaled, (3440, 1935));
        assert_eq!(m.source, rect(0, 248, 3440, 1688));
        assert_eq!(m.dest, rect(0, 0, 3440, 1440));
        let m = mapped(Fit::Fill, (4000, 1000), rect(0, 0, 1920, 1080));
        assert_eq!(m.scaled, (4320, 1080));
        assert_eq!(m.source, rect(1200, 0, 3120, 1080));
    }

    #[test]
    fn fit_leaves_bars() {
        let m = mapped(Fit::Contain, (1920, 1080), rect(0, 0, 3440, 1440));
        assert_eq!(m.scaled, (2560, 1440));
        assert_eq!(m.source, rect(0, 0, 2560, 1440));
        assert_eq!(m.dest, rect(440, 0, 3000, 1440));
    }

    #[test]
    fn stretch_and_center() {
        let full = rect(0, 0, 1920, 1080);
        let m = mapped(Fit::Stretch, (100, 50), full);
        assert_eq!(m.scaled, (1920, 1080));
        assert_eq!(m.dest, full);
        let small = mapped(Fit::Center, (100, 50), full);
        assert_eq!(small.source, rect(0, 0, 100, 50));
        assert_eq!(small.dest, rect(910, 515, 1010, 565));
        let big = mapped(Fit::Center, (4000, 2000), full);
        assert_eq!(big.source, rect(1040, 460, 2960, 1540));
        assert_eq!(big.dest, full);
    }

    #[test]
    fn works_on_monitors_away_from_the_origin() {
        let m = mapped(Fit::Fill, (3440, 1440), UPPER);
        assert_eq!(m.dest, rect(0, 0, 3440, 1440));
        assert_eq!(m.source, rect(0, 0, 3440, 1440));
    }

    #[test]
    fn zoom_and_offset_move_the_picture() {
        let monitor = rect(0, 0, 1000, 500);
        let mut view = alone(Fit::Fill, monitor);
        view.zoom = 2.0;
        // Twice as big about the middle: the middle half shows.
        let m = map(&view, (1000, 500), monitor).unwrap();
        assert_eq!(m.scaled, (2000, 1000));
        assert_eq!(m.source, rect(500, 250, 1500, 750));
        view.zoom = 1.0;
        view.offset = (100.0, -50.0);
        let m = map(&view, (1000, 500), monitor).unwrap();
        assert_eq!(m.dest, rect(100, 0, 1000, 450));
        assert_eq!(m.source, rect(0, 50, 900, 500));
        // Moved right off the monitor: nothing shows.
        view.offset = (1000.0, 0.0);
        assert_eq!(map(&view, (1000, 500), monitor), None);
    }

    #[test]
    fn span_shares_one_canvas_and_lines_up_at_the_seam() {
        let image = Wallpaper {
            path: "wide.jpg".into(),
            fit: Fit::Span,
            zoom: None,
            offset: (0.0, 0.0),
        };
        // 0.182 mm per pixel below (32" 4K), 0.233 above (34" 1440p).
        let screens = [
            Screen {
                rect: LOWER,
                image: Some(&image),
                pitch: Some(700.0 / 3840.0),
            },
            Screen {
                rect: UPPER,
                image: Some(&image),
                pitch: Some(800.0 / 3440.0),
            },
        ];
        let [lower, upper] = views(&screens)[..] else {
            panic!("two views");
        };
        let (lower, upper) = (lower.unwrap(), upper.unwrap());
        assert_eq!(lower.canvas, rect(0, -1440, 3840, 2160));
        assert_eq!(upper.canvas, lower.canvas);
        // Both zoom about the middle of the shared edge.
        assert_eq!(lower.anchor, (1946.0, 0.0));
        assert_eq!(upper.anchor, (1946.0, 0.0));
        // The finer 4K draws it bigger, so it's the same physical size.
        assert!(
            (lower.zoom - (800.0 / 3440.0) / (700.0 / 3840.0)).abs() < 1e-9
        );
        assert!((upper.zoom - 1.0).abs() < 1e-9);
        // The picture's row at the seam is the same on both: the upper
        // monitor's bottom row and the lower's top row come from the
        // same place in the picture.
        let picture = (7680, 4320);
        let below = map(&lower, picture, LOWER).unwrap();
        let above = map(&upper, picture, UPPER).unwrap();
        let seam_below =
            f64::from(below.source.top) / f64::from(below.scaled.1);
        let seam_above =
            f64::from(above.source.bottom) / f64::from(above.scaled.1);
        assert!((seam_below - seam_above).abs() < 0.001);
    }

    #[test]
    fn span_needs_the_same_picture_and_known_sizes_for_auto_zoom() {
        let one = Wallpaper {
            path: "one.jpg".into(),
            fit: Fit::Span,
            zoom: None,
            offset: (0.0, 0.0),
        };
        let other = Wallpaper {
            path: "other.jpg".into(),
            ..one.clone()
        };
        let screens = [
            Screen {
                rect: LOWER,
                image: Some(&one),
                pitch: Some(0.2),
            },
            Screen {
                rect: UPPER,
                image: Some(&other),
                pitch: None,
            },
        ];
        let found = views(&screens);
        let lower = found[0].unwrap();
        assert_eq!(lower.canvas, LOWER);
        assert!((lower.zoom - 1.0).abs() < 1e-9);
        assert_eq!(lower.anchor, (1920.0, 1080.0));
    }

    #[test]
    fn anchors_on_the_shared_edges() {
        let left = rect(0, 0, 100, 100);
        let middle = rect(100, 0, 200, 100);
        let right = rect(200, 0, 300, 100);
        assert_eq!(shared_anchor(left, &[left, middle, right]), (100.0, 50.0));
        // Shared on both sides: the middle.
        assert_eq!(
            shared_anchor(middle, &[left, middle, right]),
            (150.0, 50.0)
        );
        assert_eq!(shared_anchor(left, &[right]), (50.0, 50.0));
    }

    #[test]
    fn empty_sizes_map_to_nothing() {
        let monitor = rect(0, 0, 100, 100);
        let view = alone(Fit::Fill, monitor);
        assert_eq!(map(&view, (0, 10), monitor), None);
        let empty = View {
            canvas: rect(0, 0, 0, 100),
            ..view
        };
        assert_eq!(map(&empty, (10, 10), monitor), None);
    }

    #[test]
    fn layered_unless_a_plain_colour() {
        let plain = Backdrop {
            background: Background::Color(Rgb::BLACK),
            image: None,
        };
        assert!(!plain.layered());
        let clear = Backdrop {
            background: Background::Transparent,
            image: None,
        };
        assert!(clear.layered());
        let picture = Backdrop {
            image: Some(Wallpaper {
                path: "a.jpg".into(),
                fit: Fit::Fill,
                zoom: None,
                offset: (0.0, 0.0),
            }),
            ..plain
        };
        assert!(picture.layered());
    }
}
