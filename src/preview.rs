//! `--preview` on systems without GDI: the widgets drawn by the canvas
//! as on Windows, with the text from [`crate::font`].

use crate::canvas::Canvas;
use crate::color::Rgb;
use crate::font::Font;
use crate::widgets::{Dashboard, UNITS_PER_HEIGHT, Widget};

/// Smallest pixels-per-unit, as on Windows (`win32/paint.rs`), so text
/// stays readable on small monitors.
const MIN_SCALE: f64 = 1.0;
/// A pixel's colour bits, without its alpha.
const COLOR_BITS: u32 = 0x00FF_FFFF;

/// Renders `widget` as it'd look on a monitor `monitor_height` pixels
/// tall. Returns width, height and pixels (`0x00RRGGBB`, rows top to
/// bottom), like the Windows version. Text is left out, with a warning,
/// if no font could be found.
pub fn render(
    dashboard: &Dashboard,
    widget: Widget,
    monitor_height: u32,
    background: Rgb,
) -> Option<(u32, u32, Vec<u32>)> {
    let scale = (f64::from(monitor_height) / UNITS_PER_HEIGHT).max(MIN_SCALE);
    let (width, height) = dashboard.size(widget);
    let width = pixel_count(width * scale)?;
    let height = pixel_count(height * scale)?;
    let mut pixels = vec![0_u32; width.checked_mul(height)?];
    {
        let mut canvas = Canvas::new(width, height, &mut pixels)?;
        canvas.fill(background);
        let drawing = dashboard.draw(widget, &mut canvas, scale, background);
        match Font::find() {
            Some(font) => {
                font.draw_labels(&mut canvas, &drawing.labels, (width, height));
            }
            None => eprintln!(
                "wallflex: no font found, so the preview has no text \
                 (set WALLFLEX_FONT to a .ttf file)"
            ),
        }
    }
    for pixel in &mut pixels {
        *pixel &= COLOR_BITS;
    }
    Some((
        u32::try_from(width).ok()?,
        u32::try_from(height).ok()?,
        pixels,
    ))
}

/// A size in pixels, rounded up to whole pixels. `None` if it isn't a
/// sensible size.
#[expect(
    clippy::as_conversions,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "checked to be in 0..=u32::MAX first, so the cast is exact"
)]
fn pixel_count(value: f64) -> Option<usize> {
    let value = value.ceil();
    (value.is_finite() && (0.0..=f64::from(u32::MAX)).contains(&value))
        .then_some(value as usize)
}

#[cfg(test)]
mod tests {
    use super::{pixel_count, render};
    use crate::color::Rgb;
    use crate::widgets::{Dashboard, Widget};

    #[test]
    fn rounds_sizes_up() {
        assert_eq!(pixel_count(10.2), Some(11));
        assert_eq!(pixel_count(-1.0), None);
        assert_eq!(pixel_count(f64::INFINITY), None);
    }

    #[test]
    fn renders_the_visualizer_demo() {
        let mut dashboard = Dashboard::default();
        dashboard.show_visualizer_demo();
        let (w, h, pixels) =
            render(&dashboard, Widget::Visualizer, 1440, Rgb::BLACK).unwrap();
        assert!(w > 0 && h > 0);
        assert_eq!(pixels.len(), usize::try_from(w * h).unwrap());
        assert!(pixels.iter().any(|&p| p != 0));
    }
}
