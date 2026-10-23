//! Painting a surface's background, three ways, so their cost can be
//! compared.

use windows::Win32::Foundation::{
    COLORREF, D2DERR_RECREATE_TARGET, HWND, RECT,
};
use windows::Win32::Graphics::Direct2D::Common::{
    D2D_SIZE_U, D2D1_ALPHA_MODE_UNKNOWN, D2D1_COLOR_F, D2D1_PIXEL_FORMAT,
};
use windows::Win32::Graphics::Direct2D::{
    D2D1_FACTORY_TYPE_SINGLE_THREADED, D2D1_FEATURE_LEVEL_DEFAULT,
    D2D1_HWND_RENDER_TARGET_PROPERTIES, D2D1_PRESENT_OPTIONS_NONE,
    D2D1_RENDER_TARGET_PROPERTIES, D2D1_RENDER_TARGET_TYPE,
    D2D1_RENDER_TARGET_TYPE_HARDWARE, D2D1_RENDER_TARGET_TYPE_SOFTWARE,
    D2D1_RENDER_TARGET_USAGE_NONE, D2D1CreateFactory, ID2D1Factory,
    ID2D1HwndRenderTarget,
};
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_UNKNOWN;
use windows::Win32::Graphics::Gdi::{
    BeginPaint, CreateSolidBrush, DeleteObject, EndPaint, FillRect, HBRUSH,
    PAINTSTRUCT, ValidateRect,
};
use windows::Win32::UI::WindowsAndMessaging::GetClientRect;

use super::to_rect;
use crate::args::RenderMode;
use crate::color::Rgb;

/// Paints one surface.
pub enum Renderer {
    /// GDI fill.
    Gdi(GdiFill),
    /// Direct2D clear, on the GPU or the CPU.
    D2d(D2dFill),
}

impl Renderer {
    /// Creates a renderer for `mode` that paints `color`.
    ///
    /// # Errors
    ///
    /// When the brush or the Direct2D factory can't be created.
    pub fn new(mode: RenderMode, color: Rgb) -> windows::core::Result<Self> {
        Ok(match mode {
            RenderMode::Gdi => Self::Gdi(GdiFill::new(color)?),
            RenderMode::D2dHardware => Self::D2d(D2dFill::new(
                D2D1_RENDER_TARGET_TYPE_HARDWARE,
                color,
            )?),
            RenderMode::D2dSoftware => Self::D2d(D2dFill::new(
                D2D1_RENDER_TARGET_TYPE_SOFTWARE,
                color,
            )?),
        })
    }

    /// Handles `WM_PAINT` for `hwnd`.
    pub fn paint(&mut self, hwnd: HWND) {
        match self {
            Self::Gdi(fill) => fill.paint(hwnd),
            Self::D2d(fill) => fill.paint(hwnd),
        }
    }

    /// Handles `WM_SIZE` for `hwnd`.
    pub fn resize(&mut self, hwnd: HWND) {
        if let Self::D2d(fill) = self {
            fill.resize(hwnd);
        }
    }
}

/// A GDI solid brush.
pub struct GdiFill {
    /// The brush, deleted on drop.
    brush: HBRUSH,
}

impl GdiFill {
    /// Creates the brush.
    fn new(color: Rgb) -> windows::core::Result<Self> {
        // SAFETY: takes a plain colour value.
        let brush = unsafe { CreateSolidBrush(COLORREF(color.to_colorref())) };
        if brush.is_invalid() {
            return Err(windows::core::Error::from_thread());
        }
        Ok(Self { brush })
    }

    /// Fills the area Windows asks to repaint.
    fn paint(&self, hwnd: HWND) {
        let mut ps = PAINTSTRUCT::default();
        // SAFETY: called from `hwnd`'s own WM_PAINT handler with a valid
        // PAINTSTRUCT, and always paired with EndPaint below.
        let hdc = unsafe { BeginPaint(hwnd, &raw mut ps) };
        if !hdc.is_invalid() {
            // SAFETY: `hdc` and `ps.rcPaint` come from BeginPaint above.
            // A failed fill only leaves the area unpainted until the next
            // WM_PAINT, so the result is deliberately ignored.
            let _ = unsafe { FillRect(hdc, &raw const ps.rcPaint, self.brush) };
        }
        // SAFETY: pairs the BeginPaint above. It cannot fail.
        let _ = unsafe { EndPaint(hwnd, &raw const ps) };
    }
}

impl Drop for GdiFill {
    fn drop(&mut self) {
        // SAFETY: the brush was created by this object and is no longer
        // selected into any DC. Nothing useful can be done on failure.
        let _ = unsafe { DeleteObject(self.brush.into()) };
    }
}

/// A Direct2D render target that clears to one colour.
pub struct D2dFill {
    /// Creates the render target.
    factory: ID2D1Factory,
    /// GPU or CPU.
    kind: D2D1_RENDER_TARGET_TYPE,
    /// The clear colour.
    color: D2D1_COLOR_F,
    /// Created on first paint; dropped when the GPU device is lost.
    target: Option<ID2D1HwndRenderTarget>,
}

impl D2dFill {
    /// Creates the factory; the render target waits for the first paint.
    fn new(
        kind: D2D1_RENDER_TARGET_TYPE,
        color: Rgb,
    ) -> windows::core::Result<Self> {
        // SAFETY: no options pointer is passed; the factory is reference
        // counted by the `windows` crate.
        let factory: ID2D1Factory = unsafe {
            D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None)
        }?;
        let [r, g, b] = color.to_unit();
        Ok(Self {
            factory,
            kind,
            color: D2D1_COLOR_F { r, g, b, a: 1.0 },
            target: None,
        })
    }

    /// Clears the whole window to the colour.
    fn paint(&mut self, hwnd: HWND) {
        if self.target.is_none() {
            match self.create_target(hwnd) {
                Ok(target) => self.target = Some(target),
                Err(error) => {
                    eprintln!("wallflex: can't create render target: {error}");
                }
            }
        }
        if let Some(target) = &self.target {
            // SAFETY: BeginDraw/EndDraw are paired on the same target, on
            // the thread that created it.
            let result = unsafe {
                target.BeginDraw();
                target.Clear(Some(&raw const self.color));
                target.EndDraw(None, None)
            };
            if let Err(error) = result {
                if error.code() != D2DERR_RECREATE_TARGET {
                    eprintln!("wallflex: drawing failed: {error}");
                }
                // Either way, start again with a fresh target next time.
                self.target = None;
            }
        }
        // SAFETY: marks the window painted so WM_PAINT stops; it cannot
        // fail for a valid window.
        let _ = unsafe { ValidateRect(Some(hwnd), None) };
    }

    /// Matches the target's size to the window's.
    fn resize(&mut self, hwnd: HWND) {
        if let Some(target) = &self.target {
            let size = client_size(hwnd);
            // SAFETY: plain size value, on the target's own thread.
            if unsafe { target.Resize(&raw const size) }.is_err() {
                self.target = None;
            }
        }
    }

    /// Creates a render target covering `hwnd`.
    fn create_target(
        &self,
        hwnd: HWND,
    ) -> windows::core::Result<ID2D1HwndRenderTarget> {
        let props = D2D1_RENDER_TARGET_PROPERTIES {
            r#type: self.kind,
            pixelFormat: D2D1_PIXEL_FORMAT {
                format: DXGI_FORMAT_UNKNOWN,
                alphaMode: D2D1_ALPHA_MODE_UNKNOWN,
            },
            dpiX: 0.0,
            dpiY: 0.0,
            usage: D2D1_RENDER_TARGET_USAGE_NONE,
            minLevel: D2D1_FEATURE_LEVEL_DEFAULT,
        };
        let hwnd_props = D2D1_HWND_RENDER_TARGET_PROPERTIES {
            hwnd,
            pixelSize: client_size(hwnd),
            presentOptions: D2D1_PRESENT_OPTIONS_NONE,
        };
        // SAFETY: both property structs are valid for the call.
        unsafe {
            self.factory
                .CreateHwndRenderTarget(&raw const props, &raw const hwnd_props)
        }
    }
}

/// The window's client size in pixels; zero if it can't be read.
fn client_size(hwnd: HWND) -> D2D_SIZE_U {
    let mut rect = RECT::default();
    // SAFETY: `rect` is a valid place for the call to write to. On failure
    // it stays zero, giving a zero size.
    let _ = unsafe { GetClientRect(hwnd, &raw mut rect) };
    let r = to_rect(rect);
    D2D_SIZE_U {
        width: u32::try_from(r.width()).unwrap_or(0),
        height: u32::try_from(r.height()).unwrap_or(0),
    }
}
