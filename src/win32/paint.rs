//! GDI painting: the background colour and the sensor text panel in the
//! top-right corner. GDI keeps no buffer the size of the monitor, which
//! is what made Direct2D cost ~100 MB in milestone 1.

use windows::Win32::Foundation::{COLORREF, HWND, RECT};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, BitBlt, CLEARTYPE_QUALITY, CLIP_DEFAULT_PRECIS,
    CreateCompatibleBitmap, CreateCompatibleDC, CreateFontW, CreateSolidBrush,
    DEFAULT_CHARSET, DT_LEFT, DT_NOPREFIX, DT_SINGLELINE, DeleteDC,
    DeleteObject, DrawTextW, EndPaint, ExcludeClipRect, FillRect, GetDC,
    GetTextMetricsW, HBITMAP, HBRUSH, HDC, HFONT, HGDIOBJ, OUT_DEFAULT_PRECIS,
    PAINTSTRUCT, ReleaseDC, SRCCOPY, SelectObject, SetBkMode, SetTextColor,
    TEXTMETRICW, TRANSPARENT,
};
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::WindowsAndMessaging::GetClientRect;
use windows::core::w;

use super::to_rect;
use crate::color::Rgb;

/// Pixels per inch at 100% scaling.
const BASE_DPI: u32 = 96;
/// Points per inch, for font sizes.
const POINTS_PER_INCH: u32 = 72;
/// Panel text size, in points.
const FONT_POINTS: u32 = 11;
/// Normal font weight.
const FONT_WEIGHT_NORMAL: i32 = 400;
/// Panel width, in characters.
const PANEL_COLUMNS: i32 = 76;
/// Panel height, in lines; extra lines are cut off.
const PANEL_ROWS: i32 = 14;
/// Gap between the panel and the monitor's edges, in pixels at 100%.
const PANEL_MARGIN: i32 = 48;
/// Panel text colour.
const TEXT_COLOR: Rgb = Rgb {
    r: 0xC8,
    g: 0xC8,
    b: 0xC8,
};

/// Paints one surface.
pub struct Painter {
    /// Background brush, deleted on drop.
    background: HBRUSH,
    /// The panel font for the window's current DPI.
    font: Option<Font>,
}

/// A font and its measurements at one DPI.
struct Font {
    /// The font, deleted on drop.
    handle: HFONT,
    /// The DPI it was made for.
    dpi: u32,
    /// Average character width, in pixels (the font is monospaced).
    char_width: i32,
    /// Line height, in pixels.
    line_height: i32,
}

impl Drop for Font {
    fn drop(&mut self) {
        // SAFETY: created by `Font::create`, no longer selected into any
        // DC. Nothing useful can be done on failure.
        let _ = unsafe { DeleteObject(self.handle.into()) };
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
            background: brush,
            font: None,
        })
    }

    /// Where the text panel sits in `hwnd`, in client pixels.
    pub fn panel_rect(&mut self, hwnd: HWND) -> Option<RECT> {
        let font = self.font_for(hwnd)?;
        let mut client = RECT::default();
        // SAFETY: `client` is a valid place for the call to write to.
        unsafe { GetClientRect(hwnd, &raw mut client) }.ok()?;
        let width = font.char_width.checked_mul(PANEL_COLUMNS)?;
        let height = font.line_height.checked_mul(PANEL_ROWS)?;
        let margin = scale(PANEL_MARGIN, font.dpi);
        let left = client.right.saturating_sub(margin).saturating_sub(width);
        Some(RECT {
            left,
            top: margin,
            right: left.saturating_add(width),
            bottom: margin.saturating_add(height),
        })
    }

    /// Handles `WM_PAINT`: the panel where it needs repainting, the
    /// background everywhere else.
    pub fn paint(&mut self, hwnd: HWND, lines: &[String]) {
        let panel = self.panel_rect(hwnd);
        let mut ps = PAINTSTRUCT::default();
        // SAFETY: called from `hwnd`'s WM_PAINT handler with a valid
        // PAINTSTRUCT, and always paired with EndPaint below.
        let hdc = unsafe { BeginPaint(hwnd, &raw mut ps) };
        if !hdc.is_invalid() {
            if let (Some(panel), Some(font)) = (panel, &self.font) {
                if to_rect(panel).intersects(to_rect(ps.rcPaint)) {
                    draw_panel(hdc, panel, font, self.background, lines);
                }
                // Keep the fill below off the panel so it can't flicker.
                // SAFETY: `hdc` is valid; the result is the new clip
                // region type, not an error worth acting on.
                let _ = unsafe {
                    ExcludeClipRect(
                        hdc,
                        panel.left,
                        panel.top,
                        panel.right,
                        panel.bottom,
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

    /// The font for `hwnd`'s current DPI, recreated when the DPI changes.
    fn font_for(&mut self, hwnd: HWND) -> Option<&Font> {
        // SAFETY: only reads the window's DPI.
        let dpi = unsafe { GetDpiForWindow(hwnd) };
        if self.font.as_ref().is_none_or(|f| f.dpi != dpi) {
            self.font = Font::create(hwnd, dpi);
        }
        self.font.as_ref()
    }
}

impl Drop for Painter {
    fn drop(&mut self) {
        // SAFETY: created in `new`, not selected into any DC. Nothing
        // useful can be done on failure.
        let _ = unsafe { DeleteObject(self.background.into()) };
    }
}

impl Font {
    /// Creates the panel font for `dpi` and measures it on `hwnd`.
    fn create(hwnd: HWND, dpi: u32) -> Option<Self> {
        let pixels =
            FONT_POINTS.checked_mul(dpi)?.checked_div(POINTS_PER_INCH)?;
        // Negative height asks for the character height, not the cell.
        let height = i32::try_from(pixels).ok()?.checked_neg()?;
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
                w!("Consolas"),
            )
        };
        if handle.is_invalid() {
            return None;
        }
        let mut font = Self {
            handle,
            dpi,
            char_width: 0,
            line_height: 0,
        };
        let metrics = measure(hwnd, handle)?;
        font.char_width = metrics.tmAveCharWidth;
        font.line_height = metrics.tmHeight;
        Some(font)
    }
}

/// The font's metrics, measured on a DC for `hwnd`.
fn measure(hwnd: HWND, font: HFONT) -> Option<TEXTMETRICW> {
    // SAFETY: a DC for our own window, released below.
    let hdc = unsafe { GetDC(Some(hwnd)) };
    if hdc.is_invalid() {
        return None;
    }
    let mut metrics = TEXTMETRICW::default();
    // SAFETY: `hdc` is valid; the old font is restored before release,
    // and `metrics` is a valid place to write to.
    let ok = unsafe {
        let old = SelectObject(hdc, font.into());
        let ok = GetTextMetricsW(hdc, &raw mut metrics).as_bool();
        let _ = SelectObject(hdc, old);
        ok
    };
    // SAFETY: pairs the GetDC above. The result only says whether it was
    // released, which can't usefully be handled.
    let _ = unsafe { ReleaseDC(Some(hwnd), hdc) };
    ok.then_some(metrics)
}

/// An off-screen DC with a bitmap selected, cleaned up on drop.
struct MemoryDc {
    /// The DC.
    dc: HDC,
    /// The bitmap drawn into.
    bitmap: HBITMAP,
    /// The DC's original bitmap, restored before deleting ours.
    original: HGDIOBJ,
}

impl MemoryDc {
    /// Creates a `width` x `height` drawing area compatible with `hdc`.
    fn new(hdc: HDC, width: i32, height: i32) -> Option<Self> {
        // SAFETY: `hdc` is a valid DC from BeginPaint.
        let dc = unsafe { CreateCompatibleDC(Some(hdc)) };
        if dc.is_invalid() {
            return None;
        }
        // SAFETY: as above; the size is the panel's.
        let bitmap = unsafe { CreateCompatibleBitmap(hdc, width, height) };
        if bitmap.is_invalid() {
            // SAFETY: the DC was created just above.
            let _ = unsafe { DeleteDC(dc) };
            return None;
        }
        // SAFETY: both handles were created just above.
        let original = unsafe { SelectObject(dc, bitmap.into()) };
        Some(Self {
            dc,
            bitmap,
            original,
        })
    }
}

impl Drop for MemoryDc {
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

/// Draws the panel off-screen, then copies it to `hdc` in one go.
fn draw_panel(
    hdc: HDC,
    panel: RECT,
    font: &Font,
    background: HBRUSH,
    lines: &[String],
) {
    let area = to_rect(panel);
    let (width, height) = (area.width(), area.height());
    let Some(memory) = MemoryDc::new(hdc, width, height) else {
        return;
    };
    let whole = RECT {
        left: 0,
        top: 0,
        right: width,
        bottom: height,
    };
    // SAFETY: `memory.dc` is valid until `memory` drops; the font is
    // restored before then. Results are informational.
    unsafe {
        let _ = FillRect(memory.dc, &raw const whole, background);
        let old_font = SelectObject(memory.dc, font.handle.into());
        let _ = SetBkMode(memory.dc, TRANSPARENT);
        let _ = SetTextColor(memory.dc, colorref(TEXT_COLOR));
        for (row, line) in (0..PANEL_ROWS).zip(lines) {
            draw_line(memory.dc, line, row, font.line_height, width);
        }
        let _ = SelectObject(memory.dc, old_font);
    }
    // SAFETY: both DCs are valid and the area is inside each. A failed
    // copy only leaves the panel stale until the next paint.
    let _ = unsafe {
        BitBlt(
            hdc,
            panel.left,
            panel.top,
            width,
            height,
            Some(memory.dc),
            0,
            0,
            SRCCOPY,
        )
    };
}

/// Draws one line of text at `row`.
fn draw_line(dc: HDC, line: &str, row: i32, line_height: i32, width: i32) {
    let Some(top) = line_height.checked_mul(row) else {
        return;
    };
    let mut rect = RECT {
        left: 0,
        top,
        right: width,
        bottom: top.saturating_add(line_height),
    };
    let mut wide: Vec<u16> = line.encode_utf16().collect();
    // SAFETY: `dc` is valid, `wide` and `rect` are live. The result is
    // the text height, not an error.
    let _ = unsafe {
        DrawTextW(
            dc,
            &mut wide,
            &raw mut rect,
            DT_LEFT | DT_SINGLELINE | DT_NOPREFIX,
        )
    };
}

/// A colour as a GDI `COLORREF`.
fn colorref(color: Rgb) -> COLORREF {
    COLORREF(color.to_colorref())
}

/// Scales a size in pixels at 100% to `dpi`.
fn scale(pixels: i32, dpi: u32) -> i32 {
    i64::from(pixels)
        .checked_mul(i64::from(dpi))
        .and_then(|v| v.checked_div(i64::from(BASE_DPI)))
        .and_then(|v| i32::try_from(v).ok())
        .unwrap_or(pixels)
}
