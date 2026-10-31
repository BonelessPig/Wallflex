//! The guides widget: an open book drawn in thin ink lines. A click on it
//! opens the guides reader (`wallflex-reader.exe`); the app does that,
//! this only draws the icon and marks it clickable. It brightens, on a
//! faint rounded backdrop, while the pointer is over it.

use super::{Action, INK, Pen, QUIET};
use crate::canvas::{Area, Paint};

/// The widget's width and height.
const SIZE: f64 = 44.0;
/// The middle of the widget, where the spine is.
const MIDDLE: f64 = SIZE / 2.0;
/// Width of the book's lines.
const LINE: f64 = 1.5;
/// The book's opacity at rest.
const REST: f64 = 0.8;
/// The hover backdrop's opacity.
const HOVER_BACK: f64 = 0.1;
/// The hover backdrop's corner radius.
const HOVER_CORNER: f64 = 8.0;
/// The spine's top and bottom.
const SPINE: (f64, f64) = (13.0, 35.0);
/// The left page's outline, from the spine's top round to its bottom.
/// The right page is its mirror image.
const LEFT_PAGE: [(f64, f64); 8] = [
    (MIDDLE, SPINE.0),
    (17.0, 10.5),
    (11.0, 10.0),
    (6.0, 11.0),
    (6.0, 33.0),
    (11.0, 32.0),
    (17.0, 32.5),
    (MIDDLE, SPINE.1),
];
/// Heights of the lines of "text" on each page.
const TEXT_ROWS: [f64; 3] = [17.0, 22.0, 27.0];
/// Where the left page's text lines start and end.
const TEXT_SPAN: (f64, f64) = (10.0, 18.0);

/// The widget's size in design units.
pub fn size() -> (f64, f64) {
    (SIZE, SIZE)
}

/// Draws the book, brighter when `hovered`, and makes all of it
/// clickable.
pub fn draw(pen: &mut Pen<'_, '_>, hovered: bool) {
    let whole = Area {
        x0: 0.0,
        y0: 0.0,
        x1: SIZE,
        y1: SIZE,
    };
    let alpha = if hovered {
        pen.rounded((MIDDLE, MIDDLE), MIDDLE, HOVER_CORNER, INK, HOVER_BACK);
        1.0
    } else {
        REST
    };
    let ink = Paint { color: INK, alpha };
    pen.line_width(&LEFT_PAGE, LINE, ink);
    pen.line_width(&LEFT_PAGE.map(mirror), LINE, ink);
    pen.line_width(&[(MIDDLE, SPINE.0), (MIDDLE, SPINE.1)], LINE, ink);
    let text = Paint {
        color: INK,
        alpha: alpha * QUIET,
    };
    let (start, end) = TEXT_SPAN;
    for y in TEXT_ROWS {
        pen.line_width(&[(start, y), (end, y)], LINE, text);
        pen.line_width(&[mirror((start, y)), mirror((end, y))], LINE, text);
    }
    pen.hit(whole, Action::OpenGuides);
}

/// A point mirrored across the spine.
fn mirror((x, y): (f64, f64)) -> (f64, f64) {
    (SIZE - x, y)
}

#[cfg(test)]
mod tests {
    use super::{SIZE, mirror};
    use crate::canvas::Canvas;
    use crate::color::Rgb;
    use crate::widgets::{Action, Dashboard, Widget};

    /// Pixels per unit for the tests.
    const SCALE: f64 = 2.0;
    /// The canvas side, in pixels.
    const SIDE: usize = 88;
    /// The canvas's pixel count.
    const PIXELS: usize = SIDE * SIDE;

    /// Draws the widget, returning its pixels and clickable areas.
    fn render(dashboard: &Dashboard) -> (Vec<u32>, Vec<crate::widgets::Hit>) {
        let mut pixels = vec![0_u32; PIXELS];
        let hits = {
            let mut canvas = Canvas::new(SIDE, SIDE, &mut pixels).unwrap();
            canvas.fill(Rgb::BLACK);
            dashboard
                .draw(Widget::Guides, &mut canvas, SCALE, Rgb::BLACK)
                .hits
        };
        (pixels, hits)
    }

    #[test]
    fn is_one_clickable_square() {
        let dashboard = Dashboard::default();
        assert_eq!(dashboard.size(Widget::Guides), (SIZE, SIZE));
        let (pixels, hits) = render(&dashboard);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].action, Action::OpenGuides);
        assert!((hits[0].area.x1 - SIZE * SCALE).abs() < f64::EPSILON);
        assert!(pixels.iter().any(|&p| p != pixels[0]), "nothing drawn");
    }

    #[test]
    fn brightens_on_hover() {
        let mut dashboard = Dashboard::default();
        let (rest, _) = render(&dashboard);
        assert!(dashboard.set_hover(Some(Action::OpenGuides)));
        assert!(!dashboard.set_hover(Some(Action::OpenGuides)));
        let (hovered, _) = render(&dashboard);
        assert_ne!(rest, hovered);
        assert!(dashboard.set_hover(None));
    }

    #[test]
    fn mirrors_across_the_spine() {
        assert_eq!(mirror((6.0, 11.0)), (SIZE - 6.0, 11.0));
    }
}
