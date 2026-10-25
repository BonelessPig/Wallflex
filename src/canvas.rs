//! A small anti-aliased software canvas: rectangles, ring arcs,
//! polylines and the area under a line, drawn into a borrowed 32-bit
//! pixel buffer (in practice a GDI DIB section's memory, so nothing is
//! copied). It only ever covers the widgets, so it stays small, and it
//! needs no graphics library.
//!
//! Coordinates are in pixels as `f64`; a pixel's centre is at +0.5.
//! Edges are smoothed by how much of each pixel a shape covers.

use std::f64::consts::TAU;

use crate::color::Rgb;
use crate::image::Image;

/// A colour with an opacity, 0.0 (invisible) to 1.0 (solid).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Paint {
    /// The colour.
    pub color: Rgb,
    /// Opacity, 0.0 to 1.0.
    pub alpha: f64,
}

/// A rectangle in pixels, as floats so sizes can be fractional.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Area {
    /// Left edge.
    pub x0: f64,
    /// Top edge.
    pub y0: f64,
    /// Right edge.
    pub x1: f64,
    /// Bottom edge.
    pub y1: f64,
}

/// Draws into a pixel buffer. Each `u32` is premultiplied `0xAARRGGBB`,
/// the layout of a 32-bit GDI DIB and of a layered window's pixels.
/// After [`Canvas::fill`] everything stays opaque; after
/// [`Canvas::clear`] what isn't drawn on stays see-through.
pub struct Canvas<'a> {
    /// Width in pixels.
    width: usize,
    /// Height in pixels.
    height: usize,
    /// Rows top to bottom, `width` pixels each.
    pixels: &'a mut [u32],
    /// Only these pixel columns and rows are drawn (the whole canvas
    /// unless [`Canvas::set_clip`] narrows it).
    clip: (std::ops::Range<usize>, std::ops::Range<usize>),
}

/// Half a pixel: the distance from a pixel's edge to its centre.
const HALF: f64 = 0.5;

impl<'a> Canvas<'a> {
    /// A canvas over `pixels`, which must hold at least `width` x
    /// `height` pixels. `None` if it's too small.
    pub fn new(
        width: usize,
        height: usize,
        pixels: &'a mut [u32],
    ) -> Option<Self> {
        let count = width.checked_mul(height)?;
        let pixels = pixels.get_mut(..count)?;
        Some(Self {
            width,
            height,
            pixels,
            clip: (0..width, 0..height),
        })
    }

    /// Limits drawing to columns `x` and rows `y` (clamped to the
    /// canvas), so a small change can be redrawn without touching the
    /// rest.
    pub fn set_clip(
        &mut self,
        x: std::ops::Range<usize>,
        y: std::ops::Range<usize>,
    ) {
        let clamp = |r: std::ops::Range<usize>, limit: usize| {
            let start = r.start.min(limit);
            start..r.end.clamp(start, limit)
        };
        self.clip = (clamp(x, self.width), clamp(y, self.height));
    }

    /// Fills the canvas (inside the clip) with `color`.
    pub fn fill(&mut self, color: Rgb) {
        self.fill_with(pack(color));
    }

    /// Makes the canvas (inside the clip) fully see-through.
    pub fn clear(&mut self) {
        self.fill_with(TRANSPARENT);
    }

    /// Sets every pixel inside the clip to `value`.
    fn fill_with(&mut self, value: u32) {
        let (cols, rows) = self.clip.clone();
        if cols.len() == self.width && rows.len() == self.height {
            self.pixels.fill(value);
            return;
        }
        for y in rows {
            let start = y.checked_mul(self.width);
            let span = start.and_then(|s| {
                let from = s.checked_add(cols.start)?;
                Some(from..s.checked_add(cols.end)?)
            });
            if let Some(row) = span.and_then(|r| self.pixels.get_mut(r)) {
                row.fill(value);
            }
        }
    }

    /// Fills `area` with a colour that changes down it: `paint_at`
    /// gets each pixel row's centre (y, in pixels).
    pub fn fill_rect_rows(
        &mut self,
        area: Area,
        paint_at: impl Fn(f64) -> Paint,
    ) {
        let (rows, cols) =
            (self.span_y(area.y0, area.y1), self.span_x(area.x0, area.x1));
        for y in rows {
            let centre = index_to_f64(y) + HALF;
            let paint = paint_at(centre);
            let cover_y = overlap(index_to_f64(y), area.y0, area.y1);
            for x in cols.clone() {
                let cover_x = overlap(index_to_f64(x), area.x0, area.x1);
                self.blend(x, y, paint, cover_x * cover_y);
            }
        }
    }

    /// Fills `area`, smoothing fractional edges.
    pub fn fill_rect(&mut self, area: Area, paint: Paint) {
        let (rows, cols) =
            (self.span_y(area.y0, area.y1), self.span_x(area.x0, area.x1));
        for y in rows {
            let cover_y = overlap(index_to_f64(y), area.y0, area.y1);
            for x in cols.clone() {
                let cover_x = overlap(index_to_f64(x), area.x0, area.x1);
                self.blend(x, y, paint, cover_x * cover_y);
            }
        }
    }

    /// Draws `area`'s outline, `weight` pixels wide, inside its edges
    /// (like a CSS border).
    pub fn stroke_rect(&mut self, area: Area, weight: f64, paint: Paint) {
        let Area { x0, y0, x1, y1 } = area;
        let inner_top = y0 + weight;
        let inner_bottom = y1 - weight;
        self.fill_rect(
            Area {
                x0,
                y0,
                x1,
                y1: inner_top,
            },
            paint,
        );
        self.fill_rect(
            Area {
                x0,
                y0: inner_bottom,
                x1,
                y1,
            },
            paint,
        );
        let sides = Area {
            x0,
            y0: inner_top,
            x1: x0 + weight,
            y1: inner_bottom,
        };
        self.fill_rect(sides, paint);
        self.fill_rect(
            Area {
                x0: x1 - weight,
                x1,
                ..sides
            },
            paint,
        );
    }

    /// Draws a ring arc centred on (`cx`, `cy`), starting at the top and
    /// running clockwise for `fraction` of a full turn (0.0 to 1.0), with
    /// square ends.
    pub fn arc(
        &mut self,
        (cx, cy): (f64, f64),
        radius: f64,
        weight: f64,
        fraction: f64,
        paint: Paint,
    ) {
        if fraction <= 0.0 {
            return;
        }
        let sweep = fraction.min(1.0) * TAU;
        let outer = radius + weight / 2.0 + 1.0;
        let inner = (radius - weight / 2.0 - 1.0).max(0.0);
        for y in self.span_y(cy - outer, cy + outer) {
            let dy = index_to_f64(y) + HALF - cy;
            // Only the band between the inner and outer circles.
            let half_outer = (outer * outer - dy * dy).max(0.0).sqrt();
            let half_inner = (inner * inner - dy * dy).max(0.0).sqrt();
            let left = self.span_x(cx - half_outer, cx - half_inner + 1.0);
            let right = self.span_x(cx + half_inner - 1.0, cx + half_outer);
            let right = right.start.max(left.end)..right.end;
            for x in left.chain(right) {
                let dx = index_to_f64(x) + HALF - cx;
                let distance = dx.hypot(dy);
                let radial =
                    unit(weight / 2.0 + HALF - (distance - radius).abs());
                if radial <= 0.0 {
                    continue;
                }
                let along = if sweep >= TAU {
                    1.0
                } else {
                    arc_coverage(dx.atan2(-dy).rem_euclid(TAU), sweep, distance)
                };
                self.blend(x, y, paint, radial * along);
            }
        }
    }

    /// Draws a line through `points`, `weight` pixels wide, with round
    /// joins and ends.
    pub fn polyline(
        &mut self,
        points: &[(f64, f64)],
        weight: f64,
        paint: Paint,
    ) {
        let reach = weight / 2.0 + 1.0;
        let Some(bounds) = bounds(points, reach) else {
            return;
        };
        let rows = self.span_y(bounds.y0, bounds.y1);
        let cols = self.span_x(bounds.x0, bounds.x1);
        // Coverage per pixel in the line's box. Each segment only visits
        // the pixels near it and keeps the highest coverage, so joins
        // aren't drawn twice.
        let box_w = cols.len();
        let cells = box_w.checked_mul(rows.len()).unwrap_or(0);
        let mut coverage = vec![0.0_f64; cells];
        let cell = |x: usize, y: usize| {
            let col = x.checked_sub(cols.start)?;
            y.checked_sub(rows.start)?
                .checked_mul(box_w)?
                .checked_add(col)
        };
        for pair in points.windows(2) {
            let [a, b] = pair else {
                continue;
            };
            let near_y =
                self.span_y(a.1.min(b.1) - reach, a.1.max(b.1) + reach);
            let near_x =
                self.span_x(a.0.min(b.0) - reach, a.0.max(b.0) + reach);
            for y in near_y {
                let py = index_to_f64(y) + HALF;
                for x in near_x.clone() {
                    let px = index_to_f64(x) + HALF;
                    let distance = segment_distance((px, py), *a, *b);
                    let cover = unit(weight / 2.0 + HALF - distance);
                    if let Some(slot) =
                        cell(x, y).and_then(|i| coverage.get_mut(i))
                    {
                        *slot = slot.max(cover);
                    }
                }
            }
        }
        for y in rows.clone() {
            for x in cols.clone() {
                let cover = cell(x, y).and_then(|i| coverage.get(i)).copied();
                if let Some(cover) = cover.filter(|c| *c > 0.0) {
                    self.blend(x, y, paint, cover);
                }
            }
        }
    }

    /// Draws a graph's line through `points` (sorted left to right),
    /// `weight` pixels wide, with round joins. Unlike
    /// [`Canvas::polyline`] it never visits the whole box around the
    /// line: each pixel column only looks at the segments passing
    /// through it, so a long, spiky graph stays cheap to redraw.
    pub fn graph_line(
        &mut self,
        points: &[(f64, f64)],
        weight: f64,
        paint: Paint,
    ) {
        self.graph_line_rows(points, weight, |_| paint);
    }

    /// [`Canvas::graph_line`] with a colour that depends on height:
    /// `paint_at` gets each pixel row's centre (y, in pixels) and is
    /// asked once per row.
    pub fn graph_line_rows(
        &mut self,
        points: &[(f64, f64)],
        weight: f64,
        paint_at: impl Fn(f64) -> Paint,
    ) {
        let reach = weight / 2.0 + 1.0;
        let (Some(first), Some(last)) = (points.first(), points.last()) else {
            return;
        };
        let rows = self.row_paints(points, reach, &paint_at);
        let mut start = 0_usize;
        for x in self.span_x(first.0 - reach, last.0 + reach) {
            let px = index_to_f64(x) + HALF;
            let (left, right) = (px - reach, px + reach);
            // Skip segments that end before this column.
            while let Some([_, b]) = pair_at(points, start) {
                if b.0 >= left {
                    break;
                }
                start = start.saturating_add(1);
            }
            // The segments touching this column, and their height range
            // within it.
            let mut end = start;
            let (mut top, mut bottom) = (f64::MAX, f64::MIN);
            while let Some([a, b]) = pair_at(points, end) {
                if a.0 > right {
                    break;
                }
                for edge in [a.0.max(left), b.0.min(right)] {
                    let y = height_on(a, b, edge);
                    top = top.min(y);
                    bottom = bottom.max(y);
                }
                end = end.saturating_add(1);
            }
            if top > bottom {
                continue;
            }
            for y in self.span_y(top - reach, bottom + reach) {
                let py = index_to_f64(y) + HALF;
                let distance = (start..end)
                    .filter_map(|i| pair_at(points, i))
                    .map(|[a, b]| segment_distance((px, py), a, b))
                    .fold(f64::MAX, f64::min);
                let cover = unit(weight / 2.0 + HALF - distance);
                if cover > 0.0 {
                    let paint = rows.paint(y);
                    self.blend(x, y, paint, cover);
                }
            }
        }
    }

    /// Each pixel row's paint over the rows `points` (± `reach`) cover.
    fn row_paints(
        &self,
        points: &[(f64, f64)],
        reach: f64,
        paint_at: &impl Fn(f64) -> Paint,
    ) -> RowPaints {
        let top = points.iter().map(|p| p.1).fold(f64::MAX, f64::min);
        let bottom = points.iter().map(|p| p.1).fold(f64::MIN, f64::max);
        let span = self.span_y(top - reach, bottom + reach);
        let table = span
            .clone()
            .map(|y| paint_at(index_to_f64(y) + HALF))
            .collect();
        RowPaints {
            first: span.start,
            paints: table,
        }
    }

    /// Fills between a line through `points` (sorted left to right) and
    /// the horizontal line `base`, smoothing the top edge.
    /// `paint_at` gets each pixel row's centre (y, in pixels) and is
    /// asked once per row, so the colour can depend on height.
    pub fn fill_under_rows(
        &mut self,
        points: &[(f64, f64)],
        base: f64,
        paint_at: impl Fn(f64) -> Paint,
    ) {
        let (Some(first), Some(last)) = (points.first(), points.last()) else {
            return;
        };
        let mut edges: Vec<(f64, f64)> = points.to_vec();
        edges.push((last.0, base));
        let rows = self.row_paints(&edges, 0.0, &paint_at);
        for x in self.span_x(first.0, last.0) {
            let Some(top) = height_at(points, index_to_f64(x) + HALF) else {
                continue;
            };
            for y in self.span_y(top, base) {
                let cover = overlap(index_to_f64(y), top, base);
                let paint = rows.paint(y);
                self.blend(x, y, paint, cover);
            }
        }
    }

    /// Draws a dashed horizontal line centred on `y`.
    pub fn dashed_hline(
        &mut self,
        (x0, x1): (f64, f64),
        y: f64,
        weight: f64,
        (dash, gap): (f64, f64),
        paint: Paint,
    ) {
        if dash <= 0.0 {
            return;
        }
        let mut start = x0;
        while start < x1 {
            let area = Area {
                x0: start,
                y0: y - weight / 2.0,
                x1: (start + dash).min(x1),
                y1: y + weight / 2.0,
            };
            self.fill_rect(area, paint);
            start += dash + gap;
        }
    }

    /// Draws `image` scaled into a circle of `radius` around (`cx`,
    /// `cy`), with a smooth edge.
    pub fn image_circle(
        &mut self,
        centre: (f64, f64),
        radius: f64,
        image: &Image,
    ) {
        self.image_rounded(centre, radius, radius, image);
    }

    /// Draws `image` scaled into a square `half` from `centre` to each
    /// side, with corners rounded to `corner` and smooth edges.
    pub fn image_rounded(
        &mut self,
        (cx, cy): (f64, f64),
        half: f64,
        corner: f64,
        image: &Image,
    ) {
        if half <= 0.0 || image.width == 0 || image.height == 0 {
            return;
        }
        let scale_x = index_to_f64(image.width) / (half * 2.0);
        let scale_y = index_to_f64(image.height) / (half * 2.0);
        for y in self.span_y(cy - half - 1.0, cy + half + 1.0) {
            let dy = index_to_f64(y) + HALF - cy;
            for x in self.span_x(cx - half - 1.0, cx + half + 1.0) {
                let dx = index_to_f64(x) + HALF - cx;
                let cover =
                    unit(HALF - rounded_distance((dx, dy), half, corner));
                if cover <= 0.0 {
                    continue;
                }
                let sample = bilinear(
                    image,
                    (dx + half) * scale_x,
                    (dy + half) * scale_y,
                );
                self.blend_premultiplied(x, y, sample, cover);
            }
        }
    }

    /// Fills a square `half` from `centre` to each side, corners rounded
    /// to `corner`.
    pub fn fill_rounded(
        &mut self,
        (cx, cy): (f64, f64),
        half: f64,
        corner: f64,
        paint: Paint,
    ) {
        for y in self.span_y(cy - half - 1.0, cy + half + 1.0) {
            let dy = index_to_f64(y) + HALF - cy;
            for x in self.span_x(cx - half - 1.0, cx + half + 1.0) {
                let dx = index_to_f64(x) + HALF - cx;
                let cover =
                    unit(HALF - rounded_distance((dx, dy), half, corner));
                if cover > 0.0 {
                    self.blend(x, y, paint, cover);
                }
            }
        }
    }

    /// Lays a premultiplied (r, g, b, a) colour, each 0.0 to 1.0, over
    /// pixel (`x`, `y`) at `coverage`, if that pixel is inside the clip.
    pub fn cover(&mut self, x: usize, y: usize, rgba: [f64; 4], coverage: f64) {
        if self.clip.0.contains(&x) && self.clip.1.contains(&y) {
            self.blend_premultiplied(x, y, rgba, unit(coverage));
        }
    }

    /// Mixes a premultiplied (r, g, b, a) colour into pixel (`x`, `y`) at
    /// `coverage`.
    fn blend_premultiplied(
        &mut self,
        x: usize,
        y: usize,
        rgba: [f64; 4],
        coverage: f64,
    ) {
        let index =
            y.checked_mul(self.width).and_then(|row| row.checked_add(x));
        let Some(pixel) = index.and_then(|i| self.pixels.get_mut(i)) else {
            return;
        };
        *pixel = over(*pixel, rgba, coverage);
    }

    /// Pixel columns that `x0..x1` touches, clipped to the canvas.
    fn span_x(&self, x0: f64, x1: f64) -> std::ops::Range<usize> {
        within(clip_span(x0, x1, self.width), &self.clip.0)
    }

    /// Pixel rows that `y0..y1` touches, clipped to the canvas.
    fn span_y(&self, y0: f64, y1: f64) -> std::ops::Range<usize> {
        within(clip_span(y0, y1, self.height), &self.clip.1)
    }

    /// Mixes `paint` into pixel (`x`, `y`) at `coverage` (0.0 to 1.0).
    fn blend(&mut self, x: usize, y: usize, paint: Paint, coverage: f64) {
        let amount = unit(coverage * paint.alpha);
        if amount <= 0.0 {
            return;
        }
        let index =
            y.checked_mul(self.width).and_then(|row| row.checked_add(x));
        if let Some(pixel) = index.and_then(|i| self.pixels.get_mut(i)) {
            let channel = |c: u8| f64::from(c) / 255.0;
            let Rgb { r, g, b } = paint.color;
            let solid = [channel(r), channel(g), channel(b), 1.0];
            *pixel = over(*pixel, solid, amount);
        }
    }
}

/// `top` over `bottom` at `amount` (0.0 = all bottom, 1.0 = all top).
pub fn mix(top: Rgb, bottom: Rgb, amount: f64) -> Rgb {
    let channel = |t: u8, b: u8| {
        let (t, b) = (f64::from(t), f64::from(b));
        unit_to_u8((b + (t - b) * amount) / 255.0)
    };
    Rgb {
        r: channel(top.r, bottom.r),
        g: channel(top.g, bottom.g),
        b: channel(top.b, bottom.b),
    }
}

/// A fully see-through pixel.
const TRANSPARENT: u32 = 0;
/// A fully opaque pixel's alpha.
const OPAQUE: u8 = 255;

/// A colour as an opaque DIB pixel.
fn pack(color: Rgb) -> u32 {
    u32::from_le_bytes([color.b, color.g, color.r, OPAQUE])
}

/// A premultiplied (r, g, b, a) colour, each 0..=1, laid over `pixel`
/// at `coverage`.
fn over(pixel: u32, [red, green, blue, alpha]: [f64; 4], coverage: f64) -> u32 {
    let keep = 1.0 - alpha * coverage;
    let [old_b, old_g, old_r, old_a] = pixel.to_le_bytes();
    let channel = |src: f64, dst: u8| {
        unit_to_u8(src * coverage + f64::from(dst) / 255.0 * keep)
    };
    u32::from_le_bytes([
        channel(blue, old_b),
        channel(green, old_g),
        channel(red, old_r),
        channel(alpha, old_a),
    ])
}

/// A DIB pixel as a colour, ignoring its alpha.
#[cfg(test)]
fn unpack(pixel: u32) -> Rgb {
    let [b, g, r, _] = pixel.to_le_bytes();
    Rgb { r, g, b }
}

/// Clamps to 0.0..=1.0.
fn unit(value: f64) -> f64 {
    value.clamp(0.0, 1.0)
}

/// How much of the pixel row/column starting at `cell` lies inside
/// `start..end`.
fn overlap(cell: f64, start: f64, end: f64) -> f64 {
    unit(end.min(cell + 1.0) - start.max(cell))
}

/// How much of a pixel at `angle` (radians clockwise from the top) and
/// `distance` from the centre lies inside an arc of `sweep` radians,
/// measured along the arc so the square ends are smooth.
fn arc_coverage(angle: f64, sweep: f64, distance: f64) -> f64 {
    if angle <= sweep {
        unit(angle.min(sweep - angle) * distance + HALF)
    } else {
        let outside = (angle - sweep).min(TAU - angle) * distance;
        unit(HALF - outside)
    }
}

/// Signed distance from `(dx, dy)` (relative to the centre) to the edge
/// of a rounded square `half` to each side with corner radius `corner`:
/// negative inside.
fn rounded_distance((dx, dy): (f64, f64), half: f64, corner: f64) -> f64 {
    let corner = corner.clamp(0.0, half);
    let qx = dx.abs() - (half - corner);
    let qy = dy.abs() - (half - corner);
    let outside = qx.max(0.0).hypot(qy.max(0.0));
    outside + qx.max(qy).min(0.0) - corner
}

/// `image` sampled at (`x`, `y`) in its own pixels, blending the four
/// nearest pixels.
fn bilinear(image: &Image, x: f64, y: f64) -> [f64; 4] {
    let (x, y) = (x - HALF, y - HALF);
    let (fx, fy) = (x.floor(), y.floor());
    let (tx, ty) = (x - fx, y - fy);
    let (x0, y0) = (f64_to_index(fx), f64_to_index(fy));
    let x1 = x0.saturating_add(1).min(image.width.saturating_sub(1));
    let y1 = y0.saturating_add(1).min(image.height.saturating_sub(1));
    let corners = [
        (image.pixel(x0, y0), (1.0 - tx) * (1.0 - ty)),
        (image.pixel(x1, y0), tx * (1.0 - ty)),
        (image.pixel(x0, y1), (1.0 - tx) * ty),
        (image.pixel(x1, y1), tx * ty),
    ];
    let mut out = [0.0; 4];
    for (pixel, weight) in corners {
        for (sum, channel) in out.iter_mut().zip(pixel) {
            *sum += channel * weight;
        }
    }
    out
}

/// Paints worked out once per pixel row.
struct RowPaints {
    /// The row of the first paint.
    first: usize,
    /// One paint per row from `first`.
    paints: Vec<Paint>,
}

impl RowPaints {
    /// Row `y`'s paint (the nearest known one outside the range).
    fn paint(&self, y: usize) -> Paint {
        let index = y.saturating_sub(self.first);
        self.paints
            .get(index)
            .or_else(|| self.paints.last())
            .copied()
            .unwrap_or(Paint {
                color: Rgb {
                    r: 255,
                    g: 255,
                    b: 255,
                },
                alpha: 0.0,
            })
    }
}

/// The segment from `points[index]` to the next point, if both exist.
fn pair_at(points: &[(f64, f64)], index: usize) -> Option<[(f64, f64); 2]> {
    let pair = points.get(index..index.checked_add(2)?)?;
    <[(f64, f64); 2]>::try_from(pair).ok()
}

/// The height of the segment `a`-`b` at `x` (clamped to its ends).
fn height_on(a: (f64, f64), b: (f64, f64), x: f64) -> f64 {
    let span = b.0 - a.0;
    if span.abs() <= f64::EPSILON {
        return a.1;
    }
    let t = ((x - a.0) / span).clamp(0.0, 1.0);
    a.1 + (b.1 - a.1) * t
}

/// Distance from `p` to the segment `a`-`b`.
fn segment_distance(p: (f64, f64), a: (f64, f64), b: (f64, f64)) -> f64 {
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let length_sq = dx * dx + dy * dy;
    let t = if length_sq > 0.0 {
        unit(((p.0 - a.0) * dx + (p.1 - a.1) * dy) / length_sq)
    } else {
        0.0
    };
    (p.0 - (a.0 + t * dx)).hypot(p.1 - (a.1 + t * dy))
}

/// The y of a left-to-right line through `points` at `x`, or `None`
/// outside it.
fn height_at(points: &[(f64, f64)], x: f64) -> Option<f64> {
    points.windows(2).find_map(|pair| match pair {
        [a, b] if a.0 <= x && x <= b.0 => {
            let span = b.0 - a.0;
            let t = if span > 0.0 { (x - a.0) / span } else { 0.0 };
            Some(a.1 + (b.1 - a.1) * t)
        }
        _ => None,
    })
}

/// The box around `points`, grown by `margin`.
fn bounds(points: &[(f64, f64)], margin: f64) -> Option<Area> {
    let first = points.first()?;
    let start = Area {
        x0: first.0,
        y0: first.1,
        x1: first.0,
        y1: first.1,
    };
    let area = points.iter().fold(start, |a, &(x, y)| Area {
        x0: a.x0.min(x),
        y0: a.y0.min(y),
        x1: a.x1.max(x),
        y1: a.y1.max(y),
    });
    Some(Area {
        x0: area.x0 - margin,
        y0: area.y0 - margin,
        x1: area.x1 + margin,
        y1: area.y1 + margin,
    })
}

/// Whole pixels touched by `start..end`, clipped to `0..limit`.
fn clip_span(start: f64, end: f64, limit: usize) -> std::ops::Range<usize> {
    let first = f64_to_index(start.floor()).min(limit);
    let last = f64_to_index(end.ceil()).min(limit);
    first..last.max(first)
}

/// The part of `span` inside `clip` (empty if none).
fn within(
    span: std::ops::Range<usize>,
    clip: &std::ops::Range<usize>,
) -> std::ops::Range<usize> {
    let start = span.start.max(clip.start);
    start..span.end.min(clip.end).max(start)
}

/// A pixel index as `f64`.
fn index_to_f64(index: usize) -> f64 {
    u32::try_from(index).map_or(f64::MAX, f64::from)
}

/// A non-negative pixel position as an index; negatives become 0.
#[expect(
    clippy::as_conversions,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "clamped to 0..=u32::MAX first, so the cast is exact"
)]
fn f64_to_index(value: f64) -> usize {
    value.clamp(0.0, f64::from(u32::MAX)) as usize
}

/// 0.0..=1.0 as a colour channel.
#[expect(
    clippy::as_conversions,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "clamped and rounded to 0..=255 first, so the cast is exact"
)]
fn unit_to_u8(value: f64) -> u8 {
    (unit(value) * 255.0).round() as u8
}

#[cfg(test)]
mod tests {
    use super::{Area, Canvas, Paint, mix, unpack};
    use crate::color::Rgb;

    #[test]
    fn graph_line_matches_polyline() {
        let points = [(1.0, 18.0), (6.0, 3.0), (9.0, 15.0), (18.0, 10.0)];
        let paint = Paint {
            color: WHITE,
            alpha: 1.0,
        };
        let mut a = vec![0_u32; 400];
        let mut b = vec![0_u32; 400];
        Canvas::new(20, 20, &mut a)
            .unwrap()
            .polyline(&points, 1.5, paint);
        Canvas::new(20, 20, &mut b)
            .unwrap()
            .graph_line(&points, 1.5, paint);
        assert_eq!(a, b);
    }

    #[test]
    fn clip_limits_drawing() {
        let mut pixels = vec![0_u32; 16];
        let mut canvas = Canvas::new(4, 4, &mut pixels).unwrap();
        canvas.set_clip(1..3, 1..2);
        canvas.fill(WHITE);
        let area = Area {
            x0: 0.0,
            y0: 0.0,
            x1: 4.0,
            y1: 4.0,
        };
        canvas.fill_rect(
            area,
            Paint {
                color: WHITE,
                alpha: 1.0,
            },
        );
        let lit: Vec<usize> = (0..16).filter(|&i| pixels[i] != 0).collect();
        assert_eq!(lit, vec![5, 6]);
    }

    const WHITE: Rgb = Rgb {
        r: 255,
        g: 255,
        b: 255,
    };
    const SOLID: Paint = Paint {
        color: WHITE,
        alpha: 1.0,
    };

    fn pixel(canvas: &Canvas<'_>, x: usize, y: usize) -> u8 {
        let row = canvas.pixels.chunks_exact(canvas.width).nth(y).unwrap();
        unpack(row[x]).r
    }

    #[test]
    fn cleared_canvas_keeps_coverage_as_alpha() {
        let mut buffer = vec![0; 3];
        let mut canvas = Canvas::new(3, 1, &mut buffer).unwrap();
        canvas.fill(WHITE);
        canvas.clear();
        let area = Area {
            x0: 0.5,
            y0: 0.0,
            x1: 2.0,
            y1: 1.0,
        };
        canvas.fill_rect(area, SOLID);
        // Half covered: half the colour and half opaque (premultiplied).
        assert_eq!(buffer[0], 0x8080_8080);
        assert_eq!(buffer[1], 0xFFFF_FFFF);
        assert_eq!(buffer[2], 0);
    }

    #[test]
    fn filled_canvas_stays_opaque() {
        let mut buffer = vec![0; 1];
        let mut canvas = Canvas::new(1, 1, &mut buffer).unwrap();
        canvas.fill(Rgb::BLACK);
        let paint = Paint {
            color: WHITE,
            alpha: 0.5,
        };
        canvas.fill_rect(
            Area {
                x0: 0.0,
                y0: 0.0,
                x1: 1.0,
                y1: 1.0,
            },
            paint,
        );
        assert_eq!(buffer[0], 0xFF80_8080);
    }

    #[test]
    fn mixes_colours() {
        let grey = mix(WHITE, Rgb::BLACK, 0.5);
        assert_eq!((grey.r, grey.g, grey.b), (128, 128, 128));
    }

    #[test]
    fn fills_with_smooth_edges() {
        let mut buffer = vec![0; 4];
        let mut canvas = Canvas::new(4, 1, &mut buffer).unwrap();
        canvas.fill_rect(
            Area {
                x0: 0.5,
                y0: 0.0,
                x1: 2.0,
                y1: 1.0,
            },
            SOLID,
        );
        assert_eq!(pixel(&canvas, 0, 0), 128); // half covered
        assert_eq!(pixel(&canvas, 1, 0), 255);
        assert_eq!(pixel(&canvas, 2, 0), 0);
    }

    #[test]
    fn half_alpha_blends() {
        let mut buffer = vec![0; 1];
        let mut canvas = Canvas::new(1, 1, &mut buffer).unwrap();
        let paint = Paint {
            color: WHITE,
            alpha: 0.5,
        };
        canvas.fill_rect(
            Area {
                x0: 0.0,
                y0: 0.0,
                x1: 1.0,
                y1: 1.0,
            },
            paint,
        );
        assert_eq!(pixel(&canvas, 0, 0), 128);
    }

    #[test]
    fn quarter_arc_covers_top_right_only() {
        let mut buffer = vec![0; 1600];
        let mut canvas = Canvas::new(40, 40, &mut buffer).unwrap();
        canvas.arc((20.0, 20.0), 15.0, 2.0, 0.25, SOLID);
        assert_eq!(pixel(&canvas, 20, 5), 255, "top");
        assert_eq!(pixel(&canvas, 34, 19), 255, "right");
        assert_eq!(pixel(&canvas, 20, 34), 0, "bottom");
        assert_eq!(pixel(&canvas, 5, 19), 0, "left");
        assert_eq!(pixel(&canvas, 20, 20), 0, "centre");
    }

    #[test]
    fn full_arc_is_a_ring() {
        let mut buffer = vec![0; 1600];
        let mut canvas = Canvas::new(40, 40, &mut buffer).unwrap();
        canvas.arc((20.0, 20.0), 15.0, 2.0, 1.0, SOLID);
        assert_eq!(pixel(&canvas, 5, 19), 255, "left");
        assert_eq!(pixel(&canvas, 20, 34), 255, "bottom");
    }

    #[test]
    fn polyline_and_fill_under() {
        let mut buffer = vec![0; 100];
        let mut canvas = Canvas::new(10, 10, &mut buffer).unwrap();
        let line = [(0.0, 2.5), (10.0, 2.5)];
        canvas.polyline(&line, 1.0, SOLID);
        assert_eq!(pixel(&canvas, 5, 2), 255);
        assert_eq!(pixel(&canvas, 5, 5), 0);
        canvas.fill_under_rows(&line, 8.0, |_| SOLID);
        assert_eq!(pixel(&canvas, 5, 5), 255);
        assert_eq!(pixel(&canvas, 5, 9), 0);
    }

    #[test]
    fn stroke_leaves_inside_empty() {
        let mut buffer = vec![0; 100];
        let mut canvas = Canvas::new(10, 10, &mut buffer).unwrap();
        let area = Area {
            x0: 0.0,
            y0: 0.0,
            x1: 10.0,
            y1: 10.0,
        };
        canvas.stroke_rect(area, 1.0, SOLID);
        assert_eq!(pixel(&canvas, 0, 5), 255);
        assert_eq!(pixel(&canvas, 9, 5), 255);
        assert_eq!(pixel(&canvas, 5, 0), 255);
        assert_eq!(pixel(&canvas, 5, 5), 0);
    }

    #[test]
    fn draws_images_in_circles() {
        let image = crate::image::Image {
            width: 2,
            height: 2,
            pixels: vec![0xFFFF_FFFF; 4],
        };
        let mut buffer = vec![0; 400];
        let mut canvas = Canvas::new(20, 20, &mut buffer).unwrap();
        canvas.image_circle((10.0, 10.0), 8.0, &image);
        assert_eq!(pixel(&canvas, 10, 10), 255, "centre");
        assert_eq!(pixel(&canvas, 0, 0), 0, "outside the circle");
    }

    #[test]
    fn rejects_small_buffers() {
        let mut buffer = vec![0; 3];
        assert!(Canvas::new(2, 2, &mut buffer).is_none());
    }

    #[test]
    fn drawing_off_canvas_is_clipped() {
        let mut buffer = vec![0; 16];
        let mut canvas = Canvas::new(4, 4, &mut buffer).unwrap();
        canvas.arc((-50.0, -50.0), 10.0, 2.0, 1.0, SOLID);
        canvas.polyline(&[(-5.0, -5.0), (100.0, 100.0)], 1.0, SOLID);
        assert_eq!(pixel(&canvas, 0, 0), 255);
    }
}
