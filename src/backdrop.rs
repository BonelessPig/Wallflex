//! What's behind the widgets on a monitor: a colour or nothing (so
//! the Windows wallpaper shows through), optionally with a picture on
//! top, and the maths that fits the picture to the monitor.

use std::path::PathBuf;

use crate::color::Rgb;
use crate::geometry::Rect;

/// What fills the parts of a monitor no picture covers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Background {
    /// A solid colour.
    Color(Rgb),
    /// Nothing: the Windows wallpaper shows through.
    Transparent,
}

/// How a picture is fitted to its monitor, like Windows' own wallpaper
/// choices.
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
}

/// A picture to show behind the widgets.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Wallpaper {
    /// The image file.
    pub path: PathBuf,
    /// How it's fitted to the monitor.
    pub fit: Fit,
}

/// Everything behind the widgets on one monitor.
#[derive(Clone, Debug, PartialEq, Eq)]
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

/// Where a picture goes: which part of it, at what scale, lands where
/// on the monitor.
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

/// Works out how an `image` sized picture is fitted to a `monitor`
/// sized area (both width, height in pixels). `None` if either is
/// empty.
pub fn map(
    fit: Fit,
    image: (u32, u32),
    monitor: (i32, i32),
) -> Option<Mapping> {
    let (image_w, image_h) = image;
    let screen_w = u32::try_from(monitor.0).ok().filter(|&w| w > 0)?;
    let screen_h = u32::try_from(monitor.1).ok().filter(|&h| h > 0)?;
    if image_w == 0 || image_h == 0 {
        return None;
    }
    let wider = u64::from(image_w).checked_mul(u64::from(screen_h))?
        > u64::from(image_h).checked_mul(u64::from(screen_w))?;
    let scaled = match fit {
        // Cover: the side that's relatively shorter matches exactly.
        Fit::Fill if wider => (scale(image_w, screen_h, image_h)?, screen_h),
        Fit::Fill => (screen_w, scale(image_h, screen_w, image_w)?),
        // Contain: the side that's relatively longer matches exactly.
        Fit::Contain if wider => (screen_w, scale(image_h, screen_w, image_w)?),
        Fit::Contain => (scale(image_w, screen_h, image_h)?, screen_h),
        Fit::Stretch => (screen_w, screen_h),
        Fit::Center => (image_w, image_h),
    };
    let (source_x, dest_x, width) = centre(scaled.0, screen_w)?;
    let (source_y, dest_y, height) = centre(scaled.1, screen_h)?;
    Some(Mapping {
        scaled: (scaled.0.max(1), scaled.1.max(1)),
        source: Rect {
            left: source_x,
            top: source_y,
            right: source_x.checked_add(width)?,
            bottom: source_y.checked_add(height)?,
        },
        dest: Rect {
            left: dest_x,
            top: dest_y,
            right: dest_x.checked_add(width)?,
            bottom: dest_y.checked_add(height)?,
        },
    })
}

/// `value` x `numerator` / `denominator`, rounded, at least 1.
fn scale(value: u32, numerator: u32, denominator: u32) -> Option<u32> {
    let product = u64::from(value).checked_mul(u64::from(numerator))?;
    let half = u64::from(denominator / 2);
    let rounded = product
        .checked_add(half)?
        .checked_div(u64::from(denominator))?;
    u32::try_from(rounded.max(1)).ok()
}

/// Centres a `length` long picture along a `screen` long side: where the
/// visible part starts in the picture and on the screen, and how long
/// it is.
fn centre(length: u32, screen: u32) -> Option<(i32, i32, i32)> {
    let shown = length.min(screen);
    let crop = (length.checked_sub(shown)?) / 2;
    let margin = (screen.checked_sub(shown)?) / 2;
    Some((
        i32::try_from(crop).ok()?,
        i32::try_from(margin).ok()?,
        i32::try_from(shown).ok()?,
    ))
}

#[cfg(test)]
mod tests {
    use super::{Backdrop, Background, Fit, Mapping, Wallpaper, map};
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

    #[test]
    fn fill_covers_and_crops_the_middle() {
        // A 16:9 picture on an ultrawide: scaled to the width, the top
        // and bottom cropped.
        let m = map(Fit::Fill, (1920, 1080), (3440, 1440)).unwrap();
        assert_eq!(m.scaled, (3440, 1935));
        assert_eq!(m.source, rect(0, 247, 3440, 1687));
        assert_eq!(m.dest, rect(0, 0, 3440, 1440));
        // A wide picture on a 16:9 monitor: the sides are cropped.
        let m = map(Fit::Fill, (4000, 1000), (1920, 1080)).unwrap();
        assert_eq!(m.scaled, (4320, 1080));
        assert_eq!(m.source, rect(1200, 0, 3120, 1080));
    }

    #[test]
    fn fit_leaves_bars() {
        let m = map(Fit::Contain, (1920, 1080), (3440, 1440)).unwrap();
        assert_eq!(m.scaled, (2560, 1440));
        assert_eq!(m.source, rect(0, 0, 2560, 1440));
        assert_eq!(m.dest, rect(440, 0, 3000, 1440));
    }

    #[test]
    fn stretch_and_center() {
        let m = map(Fit::Stretch, (100, 50), (1920, 1080)).unwrap();
        assert_eq!(m.scaled, (1920, 1080));
        assert_eq!(m.dest, rect(0, 0, 1920, 1080));
        let small = map(Fit::Center, (100, 50), (1920, 1080)).unwrap();
        assert_eq!(small.source, rect(0, 0, 100, 50));
        assert_eq!(small.dest, rect(910, 515, 1010, 565));
        let big = map(Fit::Center, (4000, 2000), (1920, 1080)).unwrap();
        assert_eq!(big.source, rect(1040, 460, 2960, 1540));
        assert_eq!(big.dest, rect(0, 0, 1920, 1080));
    }

    #[test]
    fn same_shape_fills_exactly() {
        for fit in [Fit::Fill, Fit::Contain, Fit::Stretch] {
            let m = map(fit, (3840, 2160), (1920, 1080)).unwrap();
            assert_eq!(
                m,
                Mapping {
                    scaled: (1920, 1080),
                    source: rect(0, 0, 1920, 1080),
                    dest: rect(0, 0, 1920, 1080),
                },
                "{fit:?}"
            );
        }
    }

    #[test]
    fn empty_sizes_map_to_nothing() {
        assert_eq!(map(Fit::Fill, (0, 10), (100, 100)), None);
        assert_eq!(map(Fit::Fill, (10, 10), (0, 100)), None);
        assert_eq!(map(Fit::Fill, (10, 10), (100, -1)), None);
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
            }),
            ..plain
        };
        assert!(picture.layered());
    }
}
