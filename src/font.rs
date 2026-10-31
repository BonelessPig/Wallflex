//! Text for previews on macOS and Linux, where there's no GDI: the
//! widgets' [`Label`]s rasterised with `fontdue` straight into the
//! canvas. No font is shipped; the first of a list of fonts the system
//! comes with is used (or `WALLFLEX_FONT`, a path to a `.ttf`/`.otf`).

use crate::canvas::Canvas;
use crate::widgets::{Align, Label};

/// Variable naming a font file to use instead of the system's.
const FONT_VARIABLE: &str = "WALLFLEX_FONT";

/// Fonts to try, best first, as (path, index in a collection). Windows
/// draws with Bahnschrift; these are the closest plain sans fonts each
/// system ships.
#[cfg(target_os = "macos")]
const CANDIDATES: [(&str, u32); 3] = [
    ("/System/Library/Fonts/SFNS.ttf", 0),
    ("/System/Library/Fonts/Helvetica.ttc", 0),
    ("/System/Library/Fonts/Supplemental/Arial.ttf", 0),
];
/// Fonts to try, best first (see the macOS list): the usual places
/// Debian, Ubuntu, Fedora and Arch put `DejaVu`, Noto and Liberation.
#[cfg(not(target_os = "macos"))]
const CANDIDATES: [(&str, u32); 6] = [
    ("/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf", 0),
    ("/usr/share/fonts/dejavu-sans-fonts/DejaVuSans.ttf", 0),
    ("/usr/share/fonts/TTF/DejaVuSans.ttf", 0),
    ("/usr/share/fonts/truetype/noto/NotoSans-Regular.ttf", 0),
    ("/usr/share/fonts/noto/NotoSans-Regular.ttf", 0),
    (
        "/usr/share/fonts/truetype/liberation/LiberationSans-Regular.ttf",
        0,
    ),
];

/// Most a glyph's coverage byte can be.
const FULL_COVERAGE: f64 = 255.0;
/// Most a colour channel can be.
const FULL_CHANNEL: f64 = 255.0;

/// A loaded font.
pub struct Font {
    /// The parsed font.
    inner: fontdue::Font,
}

impl Font {
    /// The font named by `WALLFLEX_FONT`, else the first system font in
    /// [`CANDIDATES`] that loads. `None` if none does.
    pub fn find() -> Option<Self> {
        let chosen = std::env::var_os(FONT_VARIABLE)
            .and_then(|path| Self::load(std::path::Path::new(&path), 0));
        chosen.or_else(|| {
            CANDIDATES.iter().find_map(|(path, index)| {
                Self::load(std::path::Path::new(path), *index)
            })
        })
    }

    /// Loads the font at `path` (the `index`th in a collection).
    fn load(path: &std::path::Path, index: u32) -> Option<Self> {
        let data = std::fs::read(path).ok()?;
        let settings = fontdue::FontSettings {
            collection_index: index,
            ..fontdue::FontSettings::default()
        };
        let inner = fontdue::Font::from_bytes(data, settings).ok()?;
        Some(Self { inner })
    }

    /// Draws `labels` onto `canvas`, which is `size` (width, height)
    /// pixels. Each label's colour is drawn solid, as GDI does on an
    /// opaque widget (it's already mixed with the background).
    pub fn draw_labels(
        &self,
        canvas: &mut Canvas<'_>,
        labels: &[Label],
        size: (usize, usize),
    ) {
        for label in labels {
            if let Some(clip) = label.clip {
                canvas.set_clip(
                    pixel_floor(clip.x0)..pixel_floor(clip.x1),
                    pixel_floor(clip.y0)..pixel_floor(clip.y1),
                );
            }
            self.draw_label(canvas, label);
            if label.clip.is_some() {
                canvas.set_clip(0..size.0, 0..size.1);
            }
        }
    }

    /// Draws one label.
    fn draw_label(&self, canvas: &mut Canvas<'_>, label: &Label) {
        let px = to_f32(label.size);
        let width = self.width(&label.text, px);
        let mut pen = start_x(label.align, label.x, width).round();
        let baseline = label.y.round();
        let rgba = [
            f64::from(label.color.r) / FULL_CHANNEL,
            f64::from(label.color.g) / FULL_CHANNEL,
            f64::from(label.color.b) / FULL_CHANNEL,
            1.0,
        ];
        let mut previous = None;
        for c in label.text.chars() {
            if let Some(kern) =
                previous.and_then(|p| self.inner.horizontal_kern(p, c, px))
            {
                pen += f64::from(kern);
            }
            let (metrics, coverage) = self.inner.rasterize(c, px);
            let left = pen.round() + f64::from(metrics.xmin);
            let top =
                baseline - f64::from(metrics.ymin) - count_f64(metrics.height);
            let rows = coverage.chunks(metrics.width.max(1));
            for (dy, row) in (0_u32..).zip(rows) {
                for (dx, &amount) in (0_u32..).zip(row) {
                    let x = left + f64::from(dx);
                    let y = top + f64::from(dy);
                    if amount > 0 && x >= 0.0 && y >= 0.0 {
                        let share = f64::from(amount) / FULL_COVERAGE;
                        canvas.cover(
                            pixel_floor(x),
                            pixel_floor(y),
                            rgba,
                            share,
                        );
                    }
                }
            }
            pen += f64::from(metrics.advance_width);
            previous = Some(c);
        }
    }

    /// How far `text` reaches across at `px` pixels per em.
    fn width(&self, text: &str, px: f32) -> f64 {
        let mut width = 0.0;
        let mut previous = None;
        for c in text.chars() {
            if let Some(kern) =
                previous.and_then(|p| self.inner.horizontal_kern(p, c, px))
            {
                width += f64::from(kern);
            }
            width += f64::from(self.inner.metrics(c, px).advance_width);
            previous = Some(c);
        }
        width
    }
}

/// Where text `width` pixels wide starts, for a label at `x`.
fn start_x(align: Align, x: f64, width: f64) -> f64 {
    match align {
        Align::Left => x,
        Align::Right => x - width,
        Align::Center => x - width / 2.0,
    }
}

/// A pixel size as `fontdue` takes it.
#[expect(
    clippy::as_conversions,
    clippy::cast_possible_truncation,
    reason = "font sizes are a few hundred pixels at most; f32 holds them"
)]
fn to_f32(value: f64) -> f32 {
    value as f32
}

/// A whole-pixel count as a float.
fn count_f64(value: usize) -> f64 {
    u32::try_from(value).map_or(f64::from(u32::MAX), f64::from)
}

/// The pixel column or row containing `value` (0 for anything left of or
/// above the canvas).
#[expect(
    clippy::as_conversions,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "clamped to 0..=u32::MAX first, so the cast is exact"
)]
fn pixel_floor(value: f64) -> usize {
    value.floor().clamp(0.0, f64::from(u32::MAX)) as usize
}

#[cfg(test)]
mod tests {
    use super::{Align, Font, pixel_floor, start_x};
    use crate::canvas::Canvas;
    use crate::color::Rgb;
    use crate::widgets::Label;

    #[test]
    fn aligns_like_gdi() {
        assert!((start_x(Align::Left, 10.0, 4.0) - 10.0).abs() < 1e-9);
        assert!((start_x(Align::Right, 10.0, 4.0) - 6.0).abs() < 1e-9);
        assert!((start_x(Align::Center, 10.0, 4.0) - 8.0).abs() < 1e-9);
    }

    #[test]
    fn floors_to_pixels() {
        assert_eq!(pixel_floor(-3.5), 0);
        assert_eq!(pixel_floor(2.9), 2);
        assert_eq!(pixel_floor(f64::NAN), 0);
    }

    #[test]
    fn draws_text_when_a_font_is_found() {
        // Test machines without any of the fonts just skip this.
        let Some(font) = Font::find() else { return };
        let (w, h) = (64, 32);
        let mut pixels = vec![0_u32; w * h];
        let mut canvas = Canvas::new(w, h, &mut pixels).unwrap();
        canvas.fill(Rgb::BLACK);
        let label = Label {
            x: 2.0,
            y: 24.0,
            size: 20.0,
            color: Rgb {
                r: 0xFF,
                g: 0xFF,
                b: 0xFF,
            },
            alpha: 1.0,
            align: Align::Left,
            text: "Hi".to_owned(),
            clip: None,
        };
        font.draw_labels(&mut canvas, &[label], (w, h));
        assert!(pixels.iter().any(|p| p & 0x00FF_FFFF != 0));
    }
}
