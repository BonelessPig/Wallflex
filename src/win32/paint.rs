//! GDI painting: the background colour plus the widgets a layout places
//! on this monitor. Each widget's shapes are drawn by the software
//! [`Canvas`] straight into a DIB section sized to that widget; GDI draws
//! the text on top and copies it to the window. No buffer is ever the
//! size of the monitor, which is what made Direct2D cost ~100 MB in
//! milestone 1.
//!
//! With a transparent background or a picture, the widgets are *layered*
//! instead: each is drawn see-through around its shapes (text is drawn
//! white on black into a small mask, then blended in by its coverage)
//! and handed to its own [`LayerWindow`]. The surface itself then only
//! shows the background colour, painted once, or nothing at all when
//! it's transparent (the picture has a window of its own).

use windows::Win32::Foundation::{COLORREF, HWND, RECT, SIZE};
use windows::Win32::Graphics::Gdi::{
    ANTIALIASED_QUALITY, BI_RGB, BITMAPINFO, BITMAPINFOHEADER, BeginPaint,
    BitBlt, CLEARTYPE_QUALITY, CLIP_DEFAULT_PRECIS, CreateCompatibleDC,
    CreateDIBSection, CreateFontW, CreateRectRgn, CreateSolidBrush,
    DEFAULT_CHARSET, DIB_RGB_COLORS, DeleteDC, DeleteObject, EndPaint,
    ExcludeClipRect, FONT_QUALITY, FillRect, GdiFlush, GetDC, GetRegionData,
    GetTextExtentPoint32W, GetTextMetricsW, GetUpdateRgn, HBITMAP, HBRUSH, HDC,
    HFONT, HGDIOBJ, HRGN, IntersectClipRect, NULLREGION, OUT_DEFAULT_PRECIS,
    PAINTSTRUCT, RGN_ERROR, RGNDATA, RGNDATAHEADER, ReleaseDC, RestoreDC,
    SRCCOPY, SaveDC, SelectObject, SetBkMode, SetTextAlign, SetTextColor,
    TA_BASELINE, TA_CENTER, TA_LEFT, TA_RIGHT, TEXT_ALIGN_OPTIONS, TEXTMETRICW,
    TRANSPARENT, TextOutW,
};
use windows::core::w;

use super::layer::LayerWindow;
use super::{struct_size, to_rect, to_win32_rect};
use crate::backdrop::{Backdrop, Background};
use crate::canvas::{Area, Canvas};
use crate::color::Rgb;
use crate::config::{Layout, Placement};
use crate::geometry::{Point, Rect};
use crate::widgets::{
    Action, Align, Dashboard, Hit, Label, UNITS_PER_HEIGHT, Widget,
};

/// Smallest pixels-per-unit, so text stays readable on small monitors
/// (8-unit text would be under 8 pixels below 1000 pixels tall).
const MIN_SCALE: f64 = 1.0;
/// Normal font weight.
const FONT_WEIGHT_NORMAL: i32 = 400;
/// Fonts kept per surface; sizes beyond this are recreated as needed.
const MAX_FONTS: usize = 8;
/// Bits per pixel of the widget buffers.
const BITS_PER_PIXEL: u16 = 32;
/// How far a label's glyphs reach above its baseline, and below it, as
/// shares of its size: enough to tell whether it touches a repainted
/// area.
const LABEL_REACH: (f64, f64) = (1.0, 0.4);
/// `SaveDC`'s "most recent" index for `RestoreDC`.
const LAST_SAVED_DC: i32 = -1;
/// White, for text drawn into a mask.
const MASK_INK: Rgb = Rgb {
    r: 0xFF,
    g: 0xFF,
    b: 0xFF,
};
/// Room around text in its mask, as a share of its size, for glyphs
/// that reach past their advance.
const MASK_PAD: f64 = 0.25;
/// The mask's starting size in pixels; it grows for longer text.
const MASK_MIN: (i32, i32) = (256, 64);
/// A pixel's colour bits, without its alpha.
const COLOR_BITS: u32 = 0x00FF_FFFF;

/// Where a surface is: its monitor and the part the taskbar leaves free
/// (both in client pixels), and its top-left inside its parent window.
#[derive(Clone, Copy, Debug)]
pub struct Place {
    /// The whole monitor.
    pub bounds: Rect,
    /// The work area.
    pub work: Rect,
    /// The surface's position in its parent.
    pub origin: Point,
}

/// Paints one surface.
pub struct Painter {
    /// The background colour (black when transparent, for text
    /// opacity; nothing is filled with it then).
    background_color: Rgb,
    /// Background brush, deleted on drop.
    background: HBRUSH,
    /// Pixels per design unit on this monitor.
    scale: f64,
    /// The whole monitor, in client pixels.
    bounds: Rect,
    /// The part the taskbar leaves free, in client pixels.
    work: Rect,
    /// The placed widgets, drawn in order.
    slots: Vec<Slot>,
    /// Fonts by pixel size, shared by the widgets.
    fonts: Fonts,
    /// Whether the widgets have windows of their own.
    layered: bool,
    /// Where the surface's top-left is inside its parent, for placing
    /// the widgets' windows (which share that parent).
    origin: Point,
    /// The layered background colour still needs painting. Once
    /// painted, the window keeps it.
    backdrop_pending: bool,
}

/// One placed widget.
struct Slot {
    /// What and where.
    placement: Placement,
    /// Its pixels, kept between paints and recreated only when its size
    /// changes.
    buffer: Option<DibBuffer>,
    /// Where it was last drawn, so a move repaints the old area too.
    last: Option<Rect>,
    /// Its clickable areas from the last paint.
    hits: Vec<Hit>,
    /// Its own window, when the widgets are layered.
    layer: Option<LayerWindow>,
}

impl Painter {
    /// Creates a painter for a monitor at `place`, drawing `layout`'s
    /// widgets (none if `None`) over `backdrop`. When the backdrop is
    /// [layered](Backdrop::layered), give it the widgets' windows with
    /// [`Painter::attach_layers`] before it paints.
    ///
    /// # Errors
    ///
    /// When the background brush can't be created.
    pub fn new(
        backdrop: &Backdrop,
        place: Place,
        layout: Option<&Layout>,
    ) -> windows::core::Result<Self> {
        let layered = backdrop.layered();
        let (background, fill) = match backdrop.background {
            Background::Color(color) => (color, true),
            Background::Transparent => (Rgb::BLACK, false),
        };
        // SAFETY: takes a plain colour value.
        let brush = unsafe { CreateSolidBrush(colorref(background)) };
        if brush.is_invalid() {
            return Err(windows::core::Error::from_thread());
        }
        let size = layout.map_or(1.0, |l| l.size);
        let slots = layout
            .map(|l| l.widgets.as_slice())
            .unwrap_or_default()
            .iter()
            .map(|&placement| Slot {
                placement,
                buffer: None,
                last: None,
                hits: Vec::new(),
                layer: None,
            })
            .collect();
        let quality = if layered {
            ANTIALIASED_QUALITY
        } else {
            CLEARTYPE_QUALITY
        };
        Ok(Self {
            background_color: background,
            background: brush,
            scale: monitor_scale(place.bounds.height()) * size,
            bounds: place.bounds,
            work: place.work,
            slots,
            fonts: Fonts::new(quality),
            layered,
            origin: place.origin,
            backdrop_pending: layered && fill,
        })
    }

    /// How many widgets this monitor shows.
    pub fn widget_count(&self) -> usize {
        self.slots.len()
    }

    /// Gives the widgets their own windows (one each, in order), for a
    /// layered backdrop.
    pub fn attach_layers(&mut self, layers: Vec<LayerWindow>) {
        for (slot, layer) in self.slots.iter_mut().zip(layers) {
            slot.layer = Some(layer);
        }
    }

    /// The widgets' own windows, when layered.
    pub fn layers(&self) -> impl Iterator<Item = &LayerWindow> {
        self.slots.iter().filter_map(|slot| slot.layer.as_ref())
    }

    /// The areas to repaint after new data: where each widget is now
    /// and, if it moved or changed size, where it was.
    pub fn dirty_rects(&self, dashboard: &Dashboard) -> Vec<RECT> {
        self.changed_rects(dashboard, |_| true)
    }

    /// The same as [`Self::dirty_rects`], for just the `widget`s.
    pub fn widget_rects(
        &self,
        dashboard: &Dashboard,
        widget: Widget,
    ) -> Vec<RECT> {
        self.changed_rects(dashboard, |w| w == widget)
    }

    /// Where the widgets `wanted` picks are now, plus where they were
    /// last painted if that differs, in client pixels.
    fn changed_rects(
        &self,
        dashboard: &Dashboard,
        wanted: impl Fn(Widget) -> bool,
    ) -> Vec<RECT> {
        let mut rects = Vec::new();
        for slot in self.slots.iter().filter(|s| wanted(s.placement.widget)) {
            let now = self.slot_rect(slot.placement, dashboard);
            rects.extend(now.map(to_win32_rect));
            if slot.last != now {
                rects.extend(slot.last.map(to_win32_rect));
            }
        }
        rects
    }

    /// The areas to repaint for one frame of a glide: each widget's
    /// moving parts, in client pixels.
    pub fn moving_rects(&self, dashboard: &Dashboard) -> Vec<RECT> {
        self.rects_of(|widget| dashboard.moving_areas(widget))
    }

    /// Where the visualizer is, in client pixels.
    pub fn visualizer_rects(&self, dashboard: &Dashboard) -> Vec<RECT> {
        self.rects_of(|widget| dashboard.visualizer_areas(widget))
    }

    /// The areas a quick reading (CPU and network) changes, in client
    /// pixels.
    pub fn quick_rects(&self, dashboard: &Dashboard) -> Vec<RECT> {
        self.rects_of(|widget| dashboard.quick_areas(widget))
    }

    /// Whether this monitor shows `widget`.
    pub fn shows(&self, widget: Widget) -> bool {
        self.slots
            .iter()
            .any(|slot| slot.placement.widget == widget)
    }

    /// Where widgets with hover highlights (Discord) were last drawn, in
    /// client pixels.
    pub fn hover_rects(&self) -> Vec<Rect> {
        self.slots
            .iter()
            .filter(|slot| slot.placement.widget == Widget::Discord)
            .filter_map(|slot| slot.last)
            .collect()
    }

    /// `areas_of` each widget (in its units) as client pixels, where the
    /// widget was last drawn.
    fn rects_of(&self, areas_of: impl Fn(Widget) -> Vec<Area>) -> Vec<RECT> {
        let mut rects = Vec::new();
        for slot in &self.slots {
            let Some(rect) = slot.last else {
                continue;
            };
            for area in areas_of(slot.placement.widget) {
                let px = |units: f64, round: fn(f64) -> f64| {
                    round_px(round(units * self.scale))
                };
                let part = Rect {
                    left: rect.left.saturating_add(px(area.x0, f64::floor)),
                    top: rect.top.saturating_add(px(area.y0, f64::floor)),
                    right: rect.left.saturating_add(px(area.x1, f64::ceil)),
                    bottom: rect.top.saturating_add(px(area.y1, f64::ceil)),
                };
                rects.extend(part.intersection(rect).map(to_win32_rect));
            }
        }
        rects
    }

    /// Handles `WM_PAINT`: the parts of the widgets that need it, the
    /// background everywhere else. Only the areas Windows says changed
    /// are redrawn, so a frame that moves the bars doesn't redraw the
    /// text and graphs around them.
    pub fn paint(&mut self, hwnd: HWND, dashboard: &Dashboard) {
        if self.layered {
            self.paint_layered(hwnd, dashboard);
            return;
        }
        let rects: Vec<Option<Rect>> = self
            .slots
            .iter()
            .map(|slot| self.slot_rect(slot.placement, dashboard))
            .collect();
        let update = update_rects(hwnd);
        let mut ps = PAINTSTRUCT::default();
        // SAFETY: called from `hwnd`'s WM_PAINT handler with a valid
        // PAINTSTRUCT, and always paired with EndPaint below.
        let hdc = unsafe { BeginPaint(hwnd, &raw mut ps) };
        if !hdc.is_invalid() {
            let dirty = to_rect(ps.rcPaint);
            let changed = if update.is_empty() {
                vec![dirty]
            } else {
                update
            };
            for (slot, rect) in self.slots.iter_mut().zip(&rects) {
                if let Some(rect) = *rect {
                    let parts: Vec<Rect> = if slot.last == Some(rect) {
                        changed
                            .iter()
                            .filter_map(|c| c.intersection(rect))
                            .collect()
                    } else {
                        // Moved or resized: all of it.
                        rect.intersection(dirty)
                            .map(|_| rect)
                            .into_iter()
                            .collect()
                    };
                    let job = WidgetJob {
                        widget: slot.placement.widget,
                        scale: self.scale,
                        background: self.background_color,
                        layered: false,
                    };
                    let target = Target { hdc, rect };
                    if let Some(hits) = draw_widget(
                        target,
                        &parts,
                        &mut slot.buffer,
                        &mut self.fonts,
                        job,
                        dashboard,
                    ) {
                        slot.hits = hits;
                    }
                }
                slot.last = *rect;
            }
            for rect in rects.iter().flatten() {
                // Keep the fill below off the widgets so they can't
                // flicker.
                // SAFETY: `hdc` is valid; the result is the new clip
                // region type, not an error worth acting on.
                let _ = unsafe {
                    ExcludeClipRect(
                        hdc,
                        rect.left,
                        rect.top,
                        rect.right,
                        rect.bottom,
                    )
                };
            }
            // SAFETY: `hdc` and `rcPaint` come from BeginPaint above. A
            // failed fill only leaves the area unpainted until the next
            // WM_PAINT.
            let _ = unsafe {
                FillRect(hdc, &raw const ps.rcPaint, self.background)
            };
        }
        // SAFETY: pairs the BeginPaint above. It cannot fail.
        let _ = unsafe { EndPaint(hwnd, &raw const ps) };
    }

    /// [`Painter::paint`] for layered widgets: each widget's changed
    /// parts go to its own window, and the surface's background is
    /// painted only the first time (the window keeps it after that).
    fn paint_layered(&mut self, hwnd: HWND, dashboard: &Dashboard) {
        let rects: Vec<Option<Rect>> = self
            .slots
            .iter()
            .map(|slot| self.slot_rect(slot.placement, dashboard))
            .collect();
        let update = update_rects(hwnd);
        let mut ps = PAINTSTRUCT::default();
        // SAFETY: called from `hwnd`'s WM_PAINT handler with a valid
        // PAINTSTRUCT, and always paired with EndPaint below.
        let hdc = unsafe { BeginPaint(hwnd, &raw mut ps) };
        if !hdc.is_invalid() {
            let changed = if update.is_empty() {
                vec![to_rect(ps.rcPaint)]
            } else {
                update
            };
            for (slot, rect) in self.slots.iter_mut().zip(rects) {
                let job = WidgetJob {
                    widget: slot.placement.widget,
                    scale: self.scale,
                    background: Rgb::BLACK,
                    layered: true,
                };
                draw_layer(
                    slot,
                    rect.map(|rect| Target { hdc, rect }),
                    (&changed, self.origin),
                    &mut self.fonts,
                    job,
                    dashboard,
                );
            }
            if self.backdrop_pending {
                self.backdrop_pending = false;
                self.paint_backdrop(hdc);
            }
        }
        // SAFETY: pairs the BeginPaint above. It cannot fail.
        let _ = unsafe { EndPaint(hwnd, &raw const ps) };
    }

    /// Paints the layered background colour.
    fn paint_backdrop(&self, hdc: HDC) {
        let whole = to_win32_rect(self.bounds);
        // SAFETY: `hdc` is the paint DC; a failed fill only leaves the
        // area unpainted.
        let _ = unsafe { FillRect(hdc, &raw const whole, self.background) };
    }

    /// Whether this monitor's layout places any widgets.
    pub fn has_widgets(&self) -> bool {
        !self.slots.is_empty()
    }

    /// Frees the widget buffers and fonts. The next paint makes them
    /// again (and redraws each widget whole).
    pub fn release_buffers(&mut self) {
        for slot in &mut self.slots {
            slot.buffer = None;
        }
        self.fonts = Fonts::new(self.fonts.quality);
    }

    /// What a click at `point` (client pixels) would do, if anything.
    pub fn hit_test(&self, (x, y): (i32, i32)) -> Option<Action> {
        self.slots.iter().rev().find_map(|slot| {
            let rect = slot.last?;
            let inside = rect.left <= x
                && x < rect.right
                && rect.top <= y
                && y < rect.bottom;
            if !inside {
                return None;
            }
            let local_x = f64::from(x.saturating_sub(rect.left));
            let local_y = f64::from(y.saturating_sub(rect.top));
            slot.hits
                .iter()
                .rev()
                .find(|hit| {
                    let a = hit.area;
                    a.x0 <= local_x
                        && local_x < a.x1
                        && a.y0 <= local_y
                        && local_y < a.y1
                })
                .map(|hit| hit.action.clone())
        })
    }

    /// Where a widget goes on this monitor, or `None` if it has nothing
    /// to show.
    fn slot_rect(
        &self,
        placement: Placement,
        dashboard: &Dashboard,
    ) -> Option<Rect> {
        let (width, height) = dashboard.size(placement.widget);
        let size = (
            round_px((width * self.scale).ceil()),
            round_px((height * self.scale).ceil()),
        );
        if size.0 <= 0 || size.1 <= 0 {
            return None;
        }
        let offset = (
            round_px(placement.offset.0 * self.scale),
            round_px(placement.offset.1 * self.scale),
        );
        Some(placement.anchor.place(self.work, size, offset, self.bounds))
    }
}

impl Drop for Painter {
    fn drop(&mut self) {
        // SAFETY: created in `new`, not selected into any DC. Nothing
        // useful can be done on failure.
        let _ = unsafe { DeleteObject(self.background.into()) };
    }
}

/// Renders `widget` as it'd look on a monitor `monitor_height` pixels
/// tall, for `--preview`. Returns width, height and pixels (`0x00RRGGBB`,
/// rows top to bottom).
pub fn preview(
    dashboard: &Dashboard,
    widget: Widget,
    monitor_height: u32,
    background: Rgb,
) -> Option<(u32, u32, Vec<u32>)> {
    let height_px = i32::try_from(monitor_height).ok()?;
    let scale = monitor_scale(height_px);
    let (width, height) = dashboard.size(widget);
    let width = round_px((width * scale).ceil());
    let height = round_px((height * scale).ceil());
    let mut buffer = None;
    let mut fonts = Fonts::new(CLEARTYPE_QUALITY);
    // SAFETY: a DC for the whole screen, released below.
    let screen = unsafe { GetDC(None) };
    if screen.is_invalid() {
        return None;
    }
    let job = WidgetJob {
        widget,
        scale,
        background,
        layered: false,
    };
    let rendered = render(
        screen,
        (width, height),
        &[],
        &mut buffer,
        &mut fonts,
        job,
        dashboard,
    );
    // SAFETY: pairs the GetDC above; the result can't usefully be
    // handled.
    let _ = unsafe { ReleaseDC(None, screen) };
    rendered?;
    // Without alpha, as before: some viewers would take it as opacity,
    // and GDI's text leaves it at 0.
    let pixels = buffer
        .as_mut()?
        .pixels()
        .iter()
        .map(|pixel| pixel & COLOR_BITS)
        .collect();
    Some((
        u32::try_from(width).ok()?,
        u32::try_from(height).ok()?,
        pixels,
    ))
}

/// Pixels per design unit for a monitor `height` pixels tall.
fn monitor_scale(height: i32) -> f64 {
    (f64::from(height) / UNITS_PER_HEIGHT).max(MIN_SCALE)
}

/// What to render.
#[derive(Clone, Copy)]
struct WidgetJob {
    /// The widget.
    widget: Widget,
    /// Pixels per unit.
    scale: f64,
    /// Background colour.
    background: Rgb,
    /// See-through around its shapes, for its own window.
    layered: bool,
}

/// Where a widget goes: the window's paint DC and the widget's place.
#[derive(Clone, Copy)]
struct Target {
    /// The window's DC.
    hdc: HDC,
    /// The widget's rectangle, in client pixels.
    rect: Rect,
}

/// Renders `parts` of a widget (client pixels) off-screen, then copies
/// them to the window. Returns its clickable areas, or `None` if nothing
/// was drawn.
fn draw_widget(
    target: Target,
    parts: &[Rect],
    buffer: &mut Option<DibBuffer>,
    fonts: &mut Fonts,
    job: WidgetJob,
    dashboard: &Dashboard,
) -> Option<Vec<Hit>> {
    let Target { hdc, rect } = target;
    if parts.is_empty() {
        return None;
    }
    let origin = Point {
        x: rect.left,
        y: rect.top,
    };
    let local: Vec<Rect> =
        parts.iter().map(|p| p.relative_to(origin)).collect();
    let size = (rect.width(), rect.height());
    let (source, hits, drawn) =
        render(hdc, size, &local, buffer, fonts, job, dashboard)?;
    for part in drawn {
        // SAFETY: both DCs are valid and the area is inside each. A
        // failed copy only leaves that part stale until the next paint.
        let _ = unsafe {
            BitBlt(
                hdc,
                rect.left.saturating_add(part.left),
                rect.top.saturating_add(part.top),
                part.width(),
                part.height(),
                Some(source),
                part.left,
                part.top,
                SRCCOPY,
            )
        };
    }
    Some(hits)
}

/// Draws one layered widget into its own window: the parts of it in
/// `changed` (client pixels), or all of it if it's new, moved or
/// resized. A widget with nothing to show (no `target`) has its window
/// hidden. `origin` is the surface's top-left inside the windows'
/// parent.
fn draw_layer(
    slot: &mut Slot,
    target: Option<Target>,
    (changed, origin): (&[Rect], Point),
    fonts: &mut Fonts,
    job: WidgetJob,
    dashboard: &Dashboard,
) {
    let Some(layer) = slot.layer.as_mut() else {
        return;
    };
    let Some(Target { hdc, rect }) = target else {
        layer.hide();
        slot.last = None;
        return;
    };
    let moved = layer.place(rect.moved_by(origin));
    let whole = moved || slot.last != Some(rect);
    slot.last = Some(rect);
    let corner = Point {
        x: rect.left,
        y: rect.top,
    };
    // Empty means all of it, to `render`.
    let local: Vec<Rect> = if whole {
        Vec::new()
    } else {
        let parts: Vec<Rect> = changed
            .iter()
            .filter_map(|c| c.intersection(rect))
            .map(|part| part.relative_to(corner))
            .collect();
        if parts.is_empty() {
            return;
        }
        parts
    };
    let size = (rect.width(), rect.height());
    let kept = slot
        .buffer
        .as_ref()
        .is_some_and(|b| (b.width, b.height) == size);
    let Some((source, hits, drawn)) =
        render(hdc, size, &local, &mut slot.buffer, fonts, job, dashboard)
    else {
        return;
    };
    slot.hits = hits;
    let dirty = if whole || !kept {
        None
    } else {
        drawn.into_iter().reduce(Rect::union)
    };
    // A refused update only leaves the old pixels up until the next
    // paint.
    let _ = layer.update(source, dirty);
}

/// Renders a widget into `buffer` (recreated if the size changed):
/// shapes from the canvas, then the text on top, only inside `parts`
/// (widget pixels; all of it when empty or the buffer is new). Returns
/// the buffer's DC, the clickable areas and the parts drawn.
fn render(
    hdc: HDC,
    (width, height): (i32, i32),
    parts: &[Rect],
    buffer: &mut Option<DibBuffer>,
    fonts: &mut Fonts,
    job: WidgetJob,
    dashboard: &Dashboard,
) -> Option<(HDC, Vec<Hit>, Vec<Rect>)> {
    let w = usize::try_from(width).ok()?;
    let h = usize::try_from(height).ok()?;
    let fits = buffer
        .as_ref()
        .is_some_and(|b| b.width == width && b.height == height);
    if !fits {
        // Free the old buffer before making the new one.
        *buffer = None;
        *buffer = DibBuffer::new(hdc, width, height);
    }
    let whole = Rect {
        left: 0,
        top: 0,
        right: width,
        bottom: height,
    };
    let parts: Vec<Rect> = if fits && !parts.is_empty() {
        parts.iter().filter_map(|p| p.intersection(whole)).collect()
    } else {
        vec![whole]
    };
    let buffer = buffer.as_mut()?;
    let mut hits = Vec::new();
    for part in &parts {
        let drawing = {
            let mut canvas = Canvas::new(w, h, buffer.pixels())?;
            let span = |a: i32, b: i32| {
                usize::try_from(a).unwrap_or(0)..usize::try_from(b).unwrap_or(0)
            };
            canvas.set_clip(
                span(part.left, part.right),
                span(part.top, part.bottom),
            );
            if job.layered {
                canvas.clear();
            } else {
                canvas.fill(job.background);
            }
            dashboard.draw(job.widget, &mut canvas, job.scale, job.background)
        };
        if job.layered {
            draw_labels_layered(hdc, buffer, fonts, &drawing.labels, *part);
        } else {
            draw_labels(buffer.dc, fonts, &drawing.labels, *part);
        }
        hits = drawing.hits;
    }
    Some((buffer.dc, hits, parts))
}

/// Draws the labels that touch `part` into `dc`, clipped to it.
fn draw_labels(dc: HDC, fonts: &mut Fonts, labels: &[Label], part: Rect) {
    // SAFETY: `dc` is a buffer's memory DC. Its state is saved and
    // restored around the clip; the results are informational.
    let saved = unsafe {
        let saved = SaveDC(dc);
        let _ =
            IntersectClipRect(dc, part.left, part.top, part.right, part.bottom);
        let _ = SetBkMode(dc, TRANSPARENT);
        saved
    };
    let (top, bottom) = (f64::from(part.top), f64::from(part.bottom));
    for label in labels {
        let above = label.y - label.size * LABEL_REACH.0;
        let below = label.y + label.size * LABEL_REACH.1;
        if below < top || above > bottom {
            continue;
        }
        match label.clip {
            Some(clip) => {
                // SAFETY: as above; this label's own clip is undone
                // right after drawing it.
                unsafe {
                    let _ = SaveDC(dc);
                    let _ = IntersectClipRect(
                        dc,
                        round_px(clip.x0),
                        round_px(clip.y0),
                        round_px(clip.x1),
                        round_px(clip.y1),
                    );
                }
                fonts.draw_label(dc, label);
                // SAFETY: undoes the SaveDC just above.
                let _ = unsafe { RestoreDC(dc, LAST_SAVED_DC) };
            }
            None => fonts.draw_label(dc, label),
        }
    }
    if saved != 0 {
        // SAFETY: undoes the SaveDC above.
        let _ = unsafe { RestoreDC(dc, LAST_SAVED_DC) };
    }
}

/// Draws the labels that touch `part` onto a see-through widget's
/// `buffer`: each is drawn white on black into the fonts' mask, then
/// blended in by how much of each pixel the glyphs cover.
fn draw_labels_layered(
    hdc: HDC,
    buffer: &mut DibBuffer,
    fonts: &mut Fonts,
    labels: &[Label],
    part: Rect,
) {
    let (top, bottom) = (f64::from(part.top), f64::from(part.bottom));
    for label in labels {
        let above = label.y - label.size * LABEL_REACH.0;
        let below = label.y + label.size * LABEL_REACH.1;
        if below < top || above > bottom {
            continue;
        }
        let clip = match label.clip {
            Some(c) => part.intersection(Rect {
                left: round_px(c.x0),
                top: round_px(c.y0),
                right: round_px(c.x1),
                bottom: round_px(c.y1),
            }),
            None => Some(part),
        };
        let Some(clip) = clip else {
            continue;
        };
        let Some(spot) = fonts.rasterize(hdc, label) else {
            continue;
        };
        if let Some(mask) = fonts.mask.as_mut() {
            blend_mask(buffer, mask, (spot, clip), label);
        }
    }
}

/// Blends `label`'s colour into `buffer` wherever `mask` (placed at
/// `spot`, widget pixels) has text, inside `clip`.
fn blend_mask(
    buffer: &mut DibBuffer,
    mask: &mut DibBuffer,
    (spot, clip): (Rect, Rect),
    label: &Label,
) {
    let Some(area) = spot.intersection(clip) else {
        return;
    };
    let (Ok(width), Ok(height), Ok(mask_width)) = (
        usize::try_from(buffer.width),
        usize::try_from(buffer.height),
        usize::try_from(mask.width),
    ) else {
        return;
    };
    let coverage = mask.pixels();
    let Some(mut canvas) = Canvas::new(width, height, buffer.pixels()) else {
        return;
    };
    let channel = |c: u8| f64::from(c) / 255.0;
    let Rgb { r, g, b } = label.color;
    let ink = [channel(r), channel(g), channel(b), label.alpha];
    let index = |n: i32| usize::try_from(n).ok();
    for y in area.top..area.bottom {
        let (Some(row), Some(mask_row)) =
            (index(y), index(y.saturating_sub(spot.top)))
        else {
            continue;
        };
        let start = mask_row.checked_mul(mask_width);
        for x in area.left..area.right {
            let at = index(x.saturating_sub(spot.left))
                .zip(start)
                .and_then(|(col, start)| start.checked_add(col));
            let (Some(column), Some(&pixel)) =
                (index(x), at.and_then(|i| coverage.get(i)))
            else {
                continue;
            };
            let [blue, green, red, _] = pixel.to_le_bytes();
            let cover = channel(red.max(green).max(blue));
            if cover > 0.0 {
                canvas.cover(column, row, ink, cover);
            }
        }
    }
}

/// The rectangles Windows has marked for repainting in `hwnd` (empty if
/// it can't say). Call before `BeginPaint`, which clears them.
fn update_rects(hwnd: HWND) -> Vec<Rect> {
    // SAFETY: an empty region, deleted below.
    let region = unsafe { CreateRectRgn(0, 0, 0, 0) };
    if region.is_invalid() {
        return Vec::new();
    }
    let rects = region_rects(hwnd, region);
    // SAFETY: created above and not selected anywhere.
    let _ = unsafe { DeleteObject(region.into()) };
    rects
}

/// Copies `hwnd`'s update region into `region` and lists its
/// rectangles.
fn region_rects(hwnd: HWND, region: HRGN) -> Vec<Rect> {
    // SAFETY: valid window and region; doesn't erase anything.
    let kind = unsafe { GetUpdateRgn(hwnd, region, false) };
    if kind == NULLREGION || kind == RGN_ERROR {
        return Vec::new();
    }
    // SAFETY: no buffer, so it only returns the size needed.
    let size = unsafe { GetRegionData(region, 0, None) };
    let Ok(len) = usize::try_from(size) else {
        return Vec::new();
    };
    let header_size = std::mem::size_of::<RGNDATAHEADER>();
    if len < header_size {
        return Vec::new();
    }
    // Stored as u32s so the header and rectangles are aligned.
    let words = len.div_ceil(std::mem::size_of::<u32>());
    let mut data = vec![0_u32; words];
    // SAFETY: `data` holds at least `size` bytes.
    let written = unsafe {
        GetRegionData(region, size, Some(data.as_mut_ptr().cast::<RGNDATA>()))
    };
    if written == 0 {
        return Vec::new();
    }
    let base = data.as_ptr().cast::<u8>();
    // SAFETY: Windows wrote a header at the start of the buffer.
    let header = unsafe { base.cast::<RGNDATAHEADER>().read_unaligned() };
    let start = usize::try_from(header.dwSize).unwrap_or(header_size);
    let rect_size = std::mem::size_of::<RECT>();
    let fits = len
        .saturating_sub(start)
        .checked_div(rect_size)
        .unwrap_or(0);
    let count = usize::try_from(header.nCount).unwrap_or(0).min(fits);
    (0..count)
        .filter_map(|index| {
            let offset = index
                .checked_mul(rect_size)
                .and_then(|o| o.checked_add(start))?;
            // SAFETY: `offset + rect_size` is within the `len` bytes
            // written (checked above via `fits`).
            let rect =
                unsafe { base.add(offset).cast::<RECT>().read_unaligned() };
            Some(to_rect(rect))
        })
        .collect()
}

/// Fonts by pixel size, created on first use, and the scratch mask
/// text is drawn into for see-through widgets.
struct Fonts {
    /// The fonts made so far, by em height in pixels.
    list: Vec<(i32, Font)>,
    /// How glyph edges are smoothed: `ClearType` on a solid background,
    /// greyscale for see-through widgets (`ClearType`'s coloured edges
    /// only work over a known colour).
    quality: FONT_QUALITY,
    /// Where one label at a time is drawn white on black, made on first
    /// use and grown for longer text.
    mask: Option<DibBuffer>,
}

/// A GDI font, deleted on drop.
struct Font(HFONT);

impl Drop for Font {
    fn drop(&mut self) {
        // SAFETY: created by `Fonts::get`, no longer selected into any
        // DC. Nothing useful can be done on failure.
        let _ = unsafe { DeleteObject(self.0.into()) };
    }
}

impl Fonts {
    /// No fonts yet; they'll be made at `quality`.
    fn new(quality: FONT_QUALITY) -> Self {
        Self {
            list: Vec::new(),
            quality,
            mask: None,
        }
    }

    /// Draws `label`'s text white on black into the mask (its top-left
    /// corner), and says where that corner of the mask belongs on the
    /// widget, and how much of the mask the text uses.
    fn rasterize(&mut self, hdc: HDC, label: &Label) -> Option<Rect> {
        let font = self.get(round_px(label.size))?;
        if self.mask.is_none() {
            self.mask = DibBuffer::new(hdc, MASK_MIN.0, MASK_MIN.1);
        }
        let wide: Vec<u16> = label.text.encode_utf16().collect();
        let (extent, metrics) = measure(self.mask.as_ref()?.dc, font, &wide)?;
        let pad = round_px(label.size * MASK_PAD).max(1);
        let width = extent.cx.saturating_add(pad.saturating_mul(2));
        let height = metrics.tmHeight.saturating_add(pad.saturating_mul(2));
        let mask = self.mask_at_least(hdc, (width, height))?;
        mask.clear(width, height);
        let baseline = pad.saturating_add(metrics.tmAscent);
        // SAFETY: the mask's DC is valid; the old font is restored before
        // returning. Results are informational.
        unsafe {
            let old = SelectObject(mask.dc, font.into());
            let _ = SetTextAlign(
                mask.dc,
                TEXT_ALIGN_OPTIONS(TA_LEFT.0 | TA_BASELINE.0),
            );
            let _ = SetTextColor(mask.dc, colorref(MASK_INK));
            let _ = SetBkMode(mask.dc, TRANSPARENT);
            let _ = TextOutW(mask.dc, pad, baseline, &wide);
            let _ = SelectObject(mask.dc, old);
        }
        let shift = match label.align {
            Align::Left => 0,
            Align::Right => extent.cx,
            Align::Center => extent.cx / 2,
        };
        let left = round_px(label.x).saturating_sub(shift).saturating_sub(pad);
        let top = round_px(label.y)
            .saturating_sub(metrics.tmAscent)
            .saturating_sub(pad);
        Some(Rect {
            left,
            top,
            right: left.saturating_add(width),
            bottom: top.saturating_add(height),
        })
    }

    /// The mask, remade bigger first if it's smaller than `size`.
    fn mask_at_least(
        &mut self,
        hdc: HDC,
        (width, height): (i32, i32),
    ) -> Option<&mut DibBuffer> {
        let fits = self
            .mask
            .as_ref()
            .is_some_and(|m| m.width >= width && m.height >= height);
        if !fits {
            let (old_w, old_h) =
                self.mask.as_ref().map_or(MASK_MIN, |m| (m.width, m.height));
            // Free the old mask before making the new one.
            self.mask = None;
            self.mask =
                DibBuffer::new(hdc, width.max(old_w), height.max(old_h));
        }
        self.mask.as_mut()
    }

    /// Draws one label.
    fn draw_label(&mut self, dc: HDC, label: &Label) {
        let Some(font) = self.get(round_px(label.size)) else {
            return;
        };
        let align = match label.align {
            Align::Left => TA_LEFT,
            Align::Right => TA_RIGHT,
            Align::Center => TA_CENTER,
        };
        let wide: Vec<u16> = label.text.encode_utf16().collect();
        // SAFETY: `dc` is a valid memory DC; the old font is restored
        // before returning. Results are informational.
        unsafe {
            let old = SelectObject(dc, font.into());
            let _ =
                SetTextAlign(dc, TEXT_ALIGN_OPTIONS(align.0 | TA_BASELINE.0));
            let _ = SetTextColor(dc, colorref(label.color));
            let _ = TextOutW(dc, round_px(label.x), round_px(label.y), &wide);
            let _ = SelectObject(dc, old);
        }
    }

    /// The font at `pixels` (em height).
    fn get(&mut self, pixels: i32) -> Option<HFONT> {
        if let Some((_, font)) =
            self.list.iter().find(|(size, _)| *size == pixels)
        {
            return Some(font.0);
        }
        if self.list.len() >= MAX_FONTS {
            self.list.clear();
        }
        // Negative height asks for the character (em) height, like CSS.
        let height = pixels.checked_neg()?;
        // SAFETY: plain values and a constant face name.
        let handle = unsafe {
            CreateFontW(
                height,
                0,
                0,
                0,
                FONT_WEIGHT_NORMAL,
                0,
                0,
                0,
                DEFAULT_CHARSET,
                OUT_DEFAULT_PRECIS,
                CLIP_DEFAULT_PRECIS,
                self.quality,
                0,
                w!("Bahnschrift"),
            )
        };
        if handle.is_invalid() {
            return None;
        }
        self.list.push((pixels, Font(handle)));
        Some(handle)
    }
}

/// How big `text` is in `font`, and the font's measurements, using
/// `dc` to measure.
fn measure(dc: HDC, font: HFONT, text: &[u16]) -> Option<(SIZE, TEXTMETRICW)> {
    let mut extent = SIZE::default();
    let mut metrics = TEXTMETRICW::default();
    // SAFETY: `dc` is a valid memory DC; the old font is restored before
    // returning, and both out-parameters are locals.
    let ok = unsafe {
        let old = SelectObject(dc, font.into());
        let sized = GetTextExtentPoint32W(dc, text, &raw mut extent).as_bool();
        let measured = GetTextMetricsW(dc, &raw mut metrics).as_bool();
        let _ = SelectObject(dc, old);
        sized && measured
    };
    ok.then_some((extent, metrics))
}

/// A 32-bit DIB section selected into its own memory DC: GDI can draw
/// text into it and Rust can write its pixels directly, so nothing is
/// copied between the two. Cleaned up on drop.
struct DibBuffer {
    /// Memory DC with the bitmap selected.
    dc: HDC,
    /// The DIB section.
    bitmap: HBITMAP,
    /// The DC's original bitmap, restored before deleting ours.
    original: HGDIOBJ,
    /// The bitmap's pixels, owned by GDI.
    bits: *mut u32,
    /// Width in pixels.
    width: i32,
    /// Height in pixels.
    height: i32,
}

impl DibBuffer {
    /// Creates a `width` x `height` buffer whose DC is compatible with
    /// `hdc`.
    fn new(hdc: HDC, width: i32, height: i32) -> Option<Self> {
        let info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: struct_size::<BITMAPINFOHEADER>(),
                biWidth: width,
                // Negative: rows run top to bottom, as the canvas expects.
                biHeight: height.checked_neg()?,
                biPlanes: 1,
                biBitCount: BITS_PER_PIXEL,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bits = std::ptr::null_mut();
        // SAFETY: `info` describes the bitmap and `bits` receives the
        // address of its pixels.
        let bitmap = unsafe {
            CreateDIBSection(
                Some(hdc),
                &raw const info,
                DIB_RGB_COLORS,
                &raw mut bits,
                None,
                0,
            )
        }
        .ok()?;
        // SAFETY: `hdc` is a valid DC.
        let dc = unsafe { CreateCompatibleDC(Some(hdc)) };
        if bits.is_null() || dc.is_invalid() {
            // SAFETY: deletes what was created just above.
            unsafe {
                let _ = DeleteObject(bitmap.into());
                let _ = DeleteDC(dc);
            }
            return None;
        }
        // SAFETY: both handles were created just above.
        let original = unsafe { SelectObject(dc, bitmap.into()) };
        Some(Self {
            dc,
            bitmap,
            original,
            bits: bits.cast(),
            width,
            height,
        })
    }

    /// The pixels, for the canvas to draw into or to read back.
    fn pixels(&mut self) -> &mut [u32] {
        let count = usize::try_from(self.width)
            .ok()
            .zip(usize::try_from(self.height).ok())
            .and_then(|(w, h)| w.checked_mul(h))
            .unwrap_or(0);
        // SAFETY: GdiFlush finishes any pending GDI drawing first. `bits`
        // points to `width` x `height` 32-bit pixels that live as long as
        // the bitmap (until drop), and `&mut self` means nothing else in
        // Rust is using them.
        unsafe {
            let _ = GdiFlush();
            std::slice::from_raw_parts_mut(self.bits, count)
        }
    }
}

impl DibBuffer {
    /// Blackens the top-left `width` x `height` pixels.
    fn clear(&mut self, width: i32, height: i32) {
        let stride = usize::try_from(self.width).unwrap_or(0);
        let cols = usize::try_from(width).unwrap_or(0).min(stride);
        let rows = usize::try_from(height).unwrap_or(0);
        let pixels = self.pixels();
        for row in pixels.chunks_exact_mut(stride.max(1)).take(rows) {
            if let Some(span) = row.get_mut(..cols) {
                span.fill(0);
            }
        }
    }
}

impl Drop for DibBuffer {
    fn drop(&mut self) {
        // SAFETY: restores the original bitmap so ours can be deleted,
        // then deletes what `new` created. Failures can't be handled.
        unsafe {
            let _ = SelectObject(self.dc, self.original);
            let _ = DeleteObject(self.bitmap.into());
            let _ = DeleteDC(self.dc);
        }
    }
}

/// A colour as a GDI `COLORREF`.
fn colorref(color: Rgb) -> COLORREF {
    COLORREF(color.to_colorref())
}

/// A pixel position or size rounded to a whole pixel.
#[expect(
    clippy::as_conversions,
    clippy::cast_possible_truncation,
    reason = "clamped to the i32 range first, so the cast is exact"
)]
fn round_px(value: f64) -> i32 {
    value
        .round()
        .clamp(f64::from(i32::MIN), f64::from(i32::MAX)) as i32
}
