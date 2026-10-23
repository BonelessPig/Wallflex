//! GDI painting: the background colour plus the widgets in the
//! bottom-right corner. Shapes are drawn by the software [`Canvas`]
//! straight into a DIB section sized to the widgets only; GDI draws the
//! text on top and copies it to the window. No buffer is ever the size
//! of the monitor, which is what made Direct2D cost ~100 MB in
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
use windows::Win32::UI::WindowsAndMessaging::GetClientRect;
use windows::core::w;

use super::{struct_size, to_rect};
use crate::canvas::Canvas;
use crate::color::Rgb;
use crate::widgets::{Align, Dashboard, Label, UNITS_PER_HEIGHT};

/// Gap between the widgets and the monitor's edges, in design units.
const MARGIN: f64 = 64.0;
/// Normal font weight.
const FONT_WEIGHT_NORMAL: i32 = 400;
/// Fonts kept per surface; sizes beyond this are recreated as needed.
const MAX_FONTS: usize = 8;
/// Bits per pixel of the widget buffer.
const BITS_PER_PIXEL: u16 = 32;

/// Paints one surface.
pub struct Painter {
    /// The background colour.
    background_color: Rgb,
    /// Background brush, deleted on drop.
    background: HBRUSH,
    /// The widgets' pixels, kept between paints and recreated only when
    /// the size changes.
    buffer: Option<DibBuffer>,
    /// Fonts by pixel size.
    fonts: Vec<(i32, Font)>,
    /// Where the widgets were last drawn, so a size change repaints
    /// the old area too.
    last_block: Option<RECT>,
}

/// A GDI font, deleted on drop.
struct Font(HFONT);

impl Drop for Font {
    fn drop(&mut self) {
        // SAFETY: created by `font`, no longer selected into any DC.
        // Nothing useful can be done on failure.
        let _ = unsafe { DeleteObject(self.0.into()) };
    }
}

impl Painter {
    /// Creates a painter for the given background colour.
    ///
    /// # Errors
    ///
    /// When the brush can't be created.
    pub fn new(background: Rgb) -> windows::core::Result<Self> {
        // SAFETY: takes a plain colour value.
        let brush = unsafe { CreateSolidBrush(colorref(background)) };
        if brush.is_invalid() {
            return Err(windows::core::Error::from_thread());
        }
        Ok(Self {
            background_color: background,
            background: brush,
            buffer: None,
            fonts: Vec::new(),
            last_block: None,
        })
    }

    /// The areas to repaint after new data: where the widgets are now
    /// and, if different, where they were last drawn.
    pub fn dirty_rects(&self, hwnd: HWND, dashboard: &Dashboard) -> Vec<RECT> {
        let mut rects: Vec<RECT> = block_rect(hwnd, dashboard)
            .map(|r| r.0)
            .into_iter()
            .collect();
        if let Some(last) = self.last_block
            && rects.first() != Some(&last)
        {
            rects.push(last);
        }
        rects
    }

    /// Handles `WM_PAINT`: the widgets where they need repainting, the
    /// background everywhere else.
    pub fn paint(&mut self, hwnd: HWND, dashboard: &Dashboard) {
        let block = block_rect(hwnd, dashboard);
        let mut ps = PAINTSTRUCT::default();
        // SAFETY: called from `hwnd`'s WM_PAINT handler with a valid
        // PAINTSTRUCT, and always paired with EndPaint below.
        let hdc = unsafe { BeginPaint(hwnd, &raw mut ps) };
        if !hdc.is_invalid() {
            if let Some((rect, scale)) = block {
                if to_rect(rect).intersects(to_rect(ps.rcPaint)) {
                    self.draw_block(hdc, rect, scale, dashboard);
                }
                self.last_block = Some(rect);
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

    /// Renders the widgets as they'd look on a monitor `monitor_height`
    /// pixels tall, for `--preview`. Returns width, height and pixels
    /// (`0x00RRGGBB`, rows top to bottom).
    pub fn preview(
        &mut self,
        dashboard: &Dashboard,
        monitor_height: u32,
    ) -> Option<(u32, u32, Vec<u32>)> {
        let scale = f64::from(monitor_height) / UNITS_PER_HEIGHT;
        let (width, height) = dashboard.size();
        let width = round_px((width * scale).ceil());
        let height = round_px((height * scale).ceil());
        // SAFETY: a DC for the whole screen, released below.
        let screen = unsafe { GetDC(None) };
        if screen.is_invalid() {
            return None;
        }
        let rendered = self.render(screen, width, height, scale, dashboard);
        // SAFETY: pairs the GetDC above; the result can't usefully be
        // handled.
        let _ = unsafe { ReleaseDC(None, screen) };
        rendered?;
        let pixels = self.buffer.as_mut()?.pixels().to_vec();
        Some((
            u32::try_from(width).ok()?,
            u32::try_from(height).ok()?,
            pixels,
        ))
    }

    /// Draws the widgets off-screen, then copies them to `hdc` in one go.
    fn draw_block(
        &mut self,
        hdc: HDC,
        rect: RECT,
        scale: f64,
        dashboard: &Dashboard,
    ) {
        let area = to_rect(rect);
        let (width, height) = (area.width(), area.height());
        let Some(source) = self.render(hdc, width, height, scale, dashboard)
        else {
            return;
        };
        // SAFETY: both DCs are valid and the area is inside each. A failed
        // copy only leaves the widgets stale until the next paint.
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
    }

    /// Renders the widgets into the buffer (recreated if the size
    /// changed): shapes from the canvas, then the text on top. Returns
    /// the buffer's DC.
    fn render(
        &mut self,
        hdc: HDC,
        width: i32,
        height: i32,
        scale: f64,
        dashboard: &Dashboard,
    ) -> Option<HDC> {
        let w = usize::try_from(width).ok()?;
        let h = usize::try_from(height).ok()?;
        let fits = self
            .buffer
            .as_ref()
            .is_some_and(|b| b.width == width && b.height == height);
        if !fits {
            // Free the old buffer before making the new one.
            self.buffer = None;
            self.buffer = DibBuffer::new(hdc, width, height);
        }
        let buffer = self.buffer.as_mut()?;
        let background = self.background_color;
        let labels = {
            let mut canvas = Canvas::new(w, h, buffer.pixels())?;
            canvas.fill(background);
            dashboard.draw(&mut canvas, scale, background)
        };
        let dc = buffer.dc;
        // SAFETY: `dc` is the buffer's DC, valid while the buffer lives.
        // The result is the previous mode, not an error.
        let _ = unsafe { SetBkMode(dc, TRANSPARENT) };
        for label in &labels {
            self.draw_label(dc, label);
        }
        Some(dc)
    }

    /// Draws one label.
    fn draw_label(&mut self, dc: HDC, label: &Label) {
        let Some(font) = self.font(round_px(label.size)) else {
            return;
        };
        let align = match label.align {
            Align::Left => TA_LEFT,
            Align::Right => TA_RIGHT,
            Align::Center => TA_CENTER,
        };
        let wide: Vec<u16> = label.text.encode_utf16().collect();
        // SAFETY: `dc` is the buffer's DC; the old font is restored
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

    /// The font at `pixels` (em height), created on first use.
    fn font(&mut self, pixels: i32) -> Option<HFONT> {
        if let Some((_, font)) =
            self.fonts.iter().find(|(size, _)| *size == pixels)
        {
            return Some(font.0);
        }
        if self.fonts.len() >= MAX_FONTS {
            self.fonts.clear();
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
        self.fonts.push((pixels, Font(handle)));
        Some(handle)
    }
}

impl Drop for Painter {
    fn drop(&mut self) {
        // SAFETY: created in `new`, not selected into any DC. Nothing
        // useful can be done on failure.
        let _ = unsafe { DeleteObject(self.background.into()) };
    }
}

/// Where the widgets go in `hwnd` (bottom-right, in client pixels) and
/// the pixels per design unit, from the window's height.
fn block_rect(hwnd: HWND, dashboard: &Dashboard) -> Option<(RECT, f64)> {
    let mut client = RECT::default();
    // SAFETY: `client` is a valid place for the call to write to.
    unsafe { GetClientRect(hwnd, &raw mut client) }.ok()?;
    let screen = to_rect(client);
    let scale = f64::from(screen.height()) / UNITS_PER_HEIGHT;
    let (width, height) = dashboard.size();
    let margin = round_px(MARGIN * scale);
    let right = screen.right.saturating_sub(margin);
    let bottom = screen.bottom.saturating_sub(margin);
    let rect = RECT {
        left: right.saturating_sub(round_px((width * scale).ceil())),
        top: bottom.saturating_sub(round_px((height * scale).ceil())),
        right,
        bottom,
    };
    (scale > 0.0).then_some((rect, scale))
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
