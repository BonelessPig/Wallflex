//! GDI painting: the background colour plus the widgets a layout places
//! on this monitor. Each widget's shapes are drawn by the software
//! [`Canvas`] straight into a DIB section sized to that widget; GDI draws
//! the text on top and copies it to the window. No buffer is ever the
//! size of the monitor, which is what made Direct2D cost ~100 MB in
//! milestone 1.

use windows::Win32::Foundation::{COLORREF, HWND, RECT};
use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, BeginPaint, BitBlt,
    CLEARTYPE_QUALITY, CLIP_DEFAULT_PRECIS, CreateCompatibleDC,
    CreateDIBSection, CreateFontW, CreateRectRgn, CreateSolidBrush,
    DEFAULT_CHARSET, DIB_RGB_COLORS, DeleteDC, DeleteObject, EndPaint,
    ExcludeClipRect, FillRect, GdiFlush, GetDC, GetRegionData, GetUpdateRgn,
    HBITMAP, HBRUSH, HDC, HFONT, HGDIOBJ, HRGN, IntersectClipRect, NULLREGION,
    OUT_DEFAULT_PRECIS, PAINTSTRUCT, RGN_ERROR, RGNDATA, RGNDATAHEADER,
    ReleaseDC, RestoreDC, SRCCOPY, SaveDC, SelectObject, SetBkMode,
    SetTextAlign, SetTextColor, TA_BASELINE, TA_CENTER, TA_LEFT, TA_RIGHT,
    TEXT_ALIGN_OPTIONS, TRANSPARENT, TextOutW,
};
use windows::core::w;

use super::{struct_size, to_rect, to_win32_rect};
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

/// Paints one surface.
pub struct Painter {
    /// The background colour.
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
}

impl Painter {
    /// Creates a painter for a monitor `bounds` big (in client pixels)
    /// with `work` free of the taskbar, drawing `layout`'s widgets (none
    /// if `None`).
    ///
    /// # Errors
    ///
    /// When the background brush can't be created.
    pub fn new(
        background: Rgb,
        bounds: Rect,
        work: Rect,
        layout: Option<&Layout>,
    ) -> windows::core::Result<Self> {
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
            })
            .collect();
        Ok(Self {
            background_color: background,
            background: brush,
            scale: monitor_scale(bounds.height()) * size,
            bounds,
            work,
            slots,
            fonts: Fonts::default(),
        })
    }

    /// The areas to repaint after new data: where each widget is now
    /// and, if it moved or changed size, where it was.
    pub fn dirty_rects(&self, dashboard: &Dashboard) -> Vec<RECT> {
        let mut rects = Vec::new();
        for slot in &self.slots {
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
    pub fn visualizer_rects(&self) -> Vec<RECT> {
        self.rects_of(Dashboard::visualizer_areas)
    }

    /// The areas a quick reading (CPU and network) changes, in client
    /// pixels.
    pub fn quick_rects(&self) -> Vec<RECT> {
        self.rects_of(Dashboard::quick_areas)
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
    let mut fonts = Fonts::default();
    // SAFETY: a DC for the whole screen, released below.
    let screen = unsafe { GetDC(None) };
    if screen.is_invalid() {
        return None;
    }
    let job = WidgetJob {
        widget,
        scale,
        background,
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
    let pixels = buffer.as_mut()?.pixels().to_vec();
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
            canvas.fill(job.background);
            dashboard.draw(job.widget, &mut canvas, job.scale, job.background)
        };
        draw_labels(buffer.dc, fonts, &drawing.labels, *part);
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
        if below >= top && above <= bottom {
            fonts.draw_label(dc, label);
        }
    }
    if saved != 0 {
        // SAFETY: undoes the SaveDC above.
        let _ = unsafe { RestoreDC(dc, LAST_SAVED_DC) };
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

/// Fonts by pixel size, created on first use.
#[derive(Default)]
struct Fonts(Vec<(i32, Font)>);

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
        if let Some((_, font)) = self.0.iter().find(|(size, _)| *size == pixels)
        {
            return Some(font.0);
        }
        if self.0.len() >= MAX_FONTS {
            self.0.clear();
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
                CLEARTYPE_QUALITY,
                0,
                w!("Bahnschrift"),
            )
        };
        if handle.is_invalid() {
            return None;
        }
        self.0.push((pixels, Font(handle)));
        Some(handle)
    }
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
