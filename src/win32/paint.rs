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
    CreateDIBSection, CreateFontW, CreateSolidBrush, DEFAULT_CHARSET,
    DIB_RGB_COLORS, DeleteDC, DeleteObject, EndPaint, ExcludeClipRect,
    FillRect, GdiFlush, GetDC, HBITMAP, HBRUSH, HDC, HFONT, HGDIOBJ,
    OUT_DEFAULT_PRECIS, PAINTSTRUCT, ReleaseDC, SRCCOPY, SelectObject,
    SetBkMode, SetTextAlign, SetTextColor, TA_BASELINE, TA_CENTER, TA_LEFT,
    TA_RIGHT, TEXT_ALIGN_OPTIONS, TRANSPARENT, TextOutW,
};
use windows::core::w;

use super::{struct_size, to_rect, to_win32_rect};
use crate::canvas::Canvas;
use crate::color::Rgb;
use crate::config::{Layout, Placement};
use crate::geometry::Rect;
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

    /// Handles `WM_PAINT`: the widgets where they need repainting, the
    /// background everywhere else.
    pub fn paint(&mut self, hwnd: HWND, dashboard: &Dashboard) {
        let rects: Vec<Option<Rect>> = self
            .slots
            .iter()
            .map(|slot| self.slot_rect(slot.placement, dashboard))
            .collect();
        let mut ps = PAINTSTRUCT::default();
        // SAFETY: called from `hwnd`'s WM_PAINT handler with a valid
        // PAINTSTRUCT, and always paired with EndPaint below.
        let hdc = unsafe { BeginPaint(hwnd, &raw mut ps) };
        if !hdc.is_invalid() {
            let dirty = to_rect(ps.rcPaint);
            for (slot, rect) in self.slots.iter_mut().zip(&rects) {
                if let Some(rect) = rect.filter(|r| r.intersects(dirty)) {
                    let widget = WidgetJob {
                        widget: slot.placement.widget,
                        scale: self.scale,
                        background: self.background_color,
                    };
                    slot.hits = draw_widget(
                        hdc,
                        rect,
                        &mut slot.buffer,
                        &mut self.fonts,
                        widget,
                        dashboard,
                    );
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

/// Renders a widget off-screen, then copies it to `rect` on `hdc` in one
/// go. Returns its clickable areas.
fn draw_widget(
    hdc: HDC,
    rect: Rect,
    buffer: &mut Option<DibBuffer>,
    fonts: &mut Fonts,
    job: WidgetJob,
    dashboard: &Dashboard,
) -> Vec<Hit> {
    let (width, height) = (rect.width(), rect.height());
    let Some((source, hits)) =
        render(hdc, (width, height), buffer, fonts, job, dashboard)
    else {
        return Vec::new();
    };
    // SAFETY: both DCs are valid and the area is inside each. A failed
    // copy only leaves the widget stale until the next paint.
    let _ = unsafe {
        BitBlt(
            hdc,
            rect.left,
            rect.top,
            width,
            height,
            Some(source),
            0,
            0,
            SRCCOPY,
        )
    };
    hits
}

/// Renders a widget into `buffer` (recreated if the size changed):
/// shapes from the canvas, then the text on top. Returns the buffer's DC
/// and the clickable areas.
fn render(
    hdc: HDC,
    (width, height): (i32, i32),
    buffer: &mut Option<DibBuffer>,
    fonts: &mut Fonts,
    job: WidgetJob,
    dashboard: &Dashboard,
) -> Option<(HDC, Vec<Hit>)> {
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
    let buffer = buffer.as_mut()?;
    let drawing = {
        let mut canvas = Canvas::new(w, h, buffer.pixels())?;
        canvas.fill(job.background);
        dashboard.draw(job.widget, &mut canvas, job.scale, job.background)
    };
    let dc = buffer.dc;
    // SAFETY: `dc` is the buffer's DC, valid while the buffer lives. The
    // result is the previous mode, not an error.
    let _ = unsafe { SetBkMode(dc, TRANSPARENT) };
    for label in &drawing.labels {
        fonts.draw_label(dc, label);
    }
    Some((dc, drawing.hits))
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
