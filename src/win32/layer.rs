//! Windows for backgrounds the widgets can't simply be drawn on
//! (transparent, or a picture).
//!
//! A [`LayerWindow`] shows one widget see-through around its shapes
//! and text: a layered window given the widget's pixels with per-pixel
//! alpha. Three things shaped it:
//!
//! - Windows only takes those pixels from a buffer as big as the whole
//!   window, so it's one window per widget (sized to it), never one per
//!   monitor.
//! - Explorer's desktop window doesn't show per-pixel-alpha children,
//!   so each sits inside a *frame*: a colour-keyed child of the desktop
//!   whose own pixels are all key, so only the widget shows.
//! - A colour-keyed window costs a copy of its pixels in our memory, so
//!   frames are widget-sized; the monitor-sized surface is never keyed.
//!
//! A [`PictureWindow`] shows a picture, sized to where the picture
//! goes, painted once (the window keeps it).

use windows::Win32::Foundation::{
    COLORREF, HINSTANCE, HWND, LPARAM, LRESULT, POINT, RECT, SIZE, WPARAM,
};
use windows::Win32::Graphics::Gdi::{
    AC_SRC_ALPHA, AC_SRC_OVER, BLACK_BRUSH, BLENDFUNCTION, BeginPaint,
    EndPaint, FillRect, GetDC, GetStockObject, HBRUSH, HDC, PAINTSTRUCT,
    ReleaseDC,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, GWLP_USERDATA,
    GetWindowLongPtrW, HTTRANSPARENT, LWA_ALPHA, LWA_COLORKEY,
    RegisterClassExW, SW_HIDE, SW_SHOWNA, SWP_NOACTIVATE, SWP_NOMOVE,
    SWP_NOZORDER, SetLayeredWindowAttributes, SetWindowLongPtrW, SetWindowPos,
    ShowWindow, ULW_ALPHA, UPDATELAYEREDWINDOWINFO,
    UpdateLayeredWindowIndirect, WINDOW_EX_STYLE, WM_ERASEBKGND, WM_NCHITTEST,
    WM_PAINT, WNDCLASSEXW, WS_CHILD, WS_CLIPSIBLINGS, WS_EX_LAYERED,
    WS_EX_NOACTIVATE, WS_EX_TRANSPARENT,
};
use windows::core::{PCWSTR, w};

use super::{struct_size, to_win32_rect};
use crate::geometry::Rect;

/// Window class for frames, widgets and pictures.
const LAYER_CLASS: PCWSTR = w!("WallflexLayer");
/// Use the pixels' own alpha at full strength; also "fully opaque".
const FULL_ALPHA: u8 = 255;
/// A frame's colour key, and the colour it paints itself: black, which
/// is also what an unpainted window holds.
const KEY: COLORREF = COLORREF(0);
/// `GWLP_USERDATA` marker for windows that fill themselves with the key
/// when asked to paint. Pictures and widgets keep what they were given.
const FILLS_WITH_KEY: isize = 1;

/// One widget's window (inside its frame), destroyed on drop.
pub struct LayerWindow {
    /// The colour-keyed frame, a child of the desktop.
    frame: HWND,
    /// The widget's window, a child of the frame.
    widget: HWND,
    /// Where the frame is (parent client pixels), once placed.
    shown: Option<Rect>,
}

impl LayerWindow {
    /// Creates a hidden widget window inside `parent`.
    ///
    /// # Errors
    ///
    /// When Windows can't create it.
    pub fn create(
        parent: HWND,
        instance: HINSTANCE,
    ) -> windows::core::Result<Self> {
        let frame = create_child(parent, instance)?;
        // Built now so a failure below destroys the frame.
        let mut layer = Self {
            frame,
            widget: HWND::default(),
            shown: None,
        };
        // SAFETY: plain values on our own window; marks it as one that
        // paints itself with the key. The result is the old value.
        let _ =
            unsafe { SetWindowLongPtrW(frame, GWLP_USERDATA, FILLS_WITH_KEY) };
        // SAFETY: our own layered window; plain values.
        unsafe {
            SetLayeredWindowAttributes(
                frame,
                KEY,
                FULL_ALPHA,
                LWA_COLORKEY | LWA_ALPHA,
            )
        }?;
        layer.widget = create_child(frame, instance)?;
        // SAFETY: our own window. The result is the previous visibility,
        // not an error. The frame stays hidden until it has pixels.
        let _ = unsafe { ShowWindow(layer.widget, SW_SHOWNA) };
        Ok(layer)
    }

    /// The frame, the window that sits among the desktop's children.
    pub fn hwnd(&self) -> HWND {
        self.frame
    }

    /// Moves and sizes the window to `rect` (parent client pixels).
    /// Returns whether it changed, in which case all of it needs new
    /// pixels.
    pub fn place(&mut self, rect: Rect) -> bool {
        if self.shown == Some(rect) {
            return false;
        }
        let flags = SWP_NOZORDER | SWP_NOACTIVATE;
        // SAFETY: our own windows; the z-order is kept.
        let moved = unsafe {
            SetWindowPos(
                self.frame,
                None,
                rect.left,
                rect.top,
                rect.width(),
                rect.height(),
                flags,
            )
            .and_then(|()| {
                SetWindowPos(
                    self.widget,
                    None,
                    0,
                    0,
                    rect.width(),
                    rect.height(),
                    flags | SWP_NOMOVE,
                )
            })
        };
        if moved.is_ok() {
            self.shown = Some(rect);
        }
        true
    }

    /// Hands the window the pixels in `source` (a DC whose bitmap is
    /// exactly the window's size, premultiplied BGRA), redrawing only
    /// `dirty` (window pixels) if given, and shows it. Returns whether
    /// Windows took them.
    pub fn update(&self, source: HDC, dirty: Option<Rect>) -> bool {
        let Some(rect) = self.shown else {
            return false;
        };
        let size = SIZE {
            cx: rect.width(),
            cy: rect.height(),
        };
        let origin = POINT::default();
        let blend = BLENDFUNCTION {
            BlendOp: u8::try_from(AC_SRC_OVER).unwrap_or_default(),
            BlendFlags: 0,
            SourceConstantAlpha: FULL_ALPHA,
            AlphaFormat: u8::try_from(AC_SRC_ALPHA).unwrap_or_default(),
        };
        let dirty: Option<RECT> = dirty.map(to_win32_rect);
        let info = UPDATELAYEREDWINDOWINFO {
            cbSize: struct_size::<UPDATELAYEREDWINDOWINFO>(),
            psize: &raw const size,
            hdcSrc: source,
            pptSrc: &raw const origin,
            pblend: &raw const blend,
            dwFlags: ULW_ALPHA,
            prcDirty: dirty
                .as_ref()
                .map_or(std::ptr::null(), std::ptr::from_ref),
            ..Default::default()
        };
        // SAFETY: every pointer in `info` points to a local that outlives
        // the call, and `source` is a valid DC the caller owns.
        let ok = unsafe {
            UpdateLayeredWindowIndirect(self.widget, &raw const info)
        }
        .as_bool();
        if ok {
            // SAFETY: our own window. The result is the previous
            // visibility, not an error.
            let _ = unsafe { ShowWindow(self.frame, SW_SHOWNA) };
        }
        ok
    }

    /// Hides the window, for a widget with nothing to show.
    pub fn hide(&mut self) {
        if self.shown.take().is_some() {
            // SAFETY: our own window. The result is the previous
            // visibility, not an error.
            let _ = unsafe { ShowWindow(self.frame, SW_HIDE) };
        }
    }
}

impl Drop for LayerWindow {
    fn drop(&mut self) {
        // SAFETY: the frame belongs to this object, and destroying it
        // destroys the widget window inside. It may already be gone if
        // Explorer restarted, so failure is expected and ignored.
        let _ = unsafe { DestroyWindow(self.frame) };
    }
}

/// A picture behind the widgets, destroyed on drop.
pub struct PictureWindow {
    /// The window, a child of the desktop.
    hwnd: HWND,
}

impl PictureWindow {
    /// Creates a hidden window covering `rect` (`parent` client pixels).
    /// [`PictureWindow::show`] shows it with the picture.
    ///
    /// # Errors
    ///
    /// When Windows can't create or place it.
    pub fn create(
        parent: HWND,
        instance: HINSTANCE,
        rect: Rect,
    ) -> Result<Self, String> {
        let hwnd = create_child(parent, instance).map_err(|e| e.to_string())?;
        let picture = Self { hwnd };
        // SAFETY: our own window; plain values.
        unsafe {
            SetLayeredWindowAttributes(hwnd, KEY, FULL_ALPHA, LWA_ALPHA)
                .and_then(|()| {
                    SetWindowPos(
                        hwnd,
                        None,
                        rect.left,
                        rect.top,
                        rect.width(),
                        rect.height(),
                        SWP_NOZORDER | SWP_NOACTIVATE,
                    )
                })
        }
        .map_err(|e| e.to_string())?;
        Ok(picture)
    }

    /// Shows the window with what `paint` draws into it (with (0, 0) at
    /// its top-left). It's painted once shown, since a layered window
    /// only keeps what's drawn while it's visible; after that the
    /// window keeps it.
    ///
    /// # Errors
    ///
    /// `paint`'s message when it fails.
    pub fn show(
        &self,
        paint: impl FnOnce(HDC) -> Result<(), String>,
    ) -> Result<(), String> {
        // SAFETY: our own window. The result is the previous visibility,
        // not an error.
        let _ = unsafe { ShowWindow(self.hwnd, SW_SHOWNA) };
        // SAFETY: our own window; released below.
        let dc = unsafe { GetDC(Some(self.hwnd)) };
        if dc.is_invalid() {
            return Err("no DC to paint the picture with".into());
        }
        let painted = paint(dc);
        // SAFETY: pairs the GetDC above. The result can't usefully be
        // handled.
        let _ = unsafe { ReleaseDC(Some(self.hwnd), dc) };
        painted
    }

    /// The window.
    pub fn hwnd(&self) -> HWND {
        self.hwnd
    }
}

impl Drop for PictureWindow {
    fn drop(&mut self) {
        // SAFETY: the window belongs to this object. It may already be
        // gone if Explorer restarted, so failure is expected and ignored.
        let _ = unsafe { DestroyWindow(self.hwnd) };
    }
}

/// Creates a hidden, layered child of `parent` that clicks pass
/// through.
fn create_child(
    parent: HWND,
    instance: HINSTANCE,
) -> windows::core::Result<HWND> {
    let ex_style: WINDOW_EX_STYLE =
        WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_NOACTIVATE;
    // SAFETY: the class is registered and the parent is a live window.
    unsafe {
        CreateWindowExW(
            ex_style,
            LAYER_CLASS,
            w!("Wallflex"),
            WS_CHILD | WS_CLIPSIBLINGS,
            0,
            0,
            0,
            0,
            Some(parent),
            None,
            Some(instance),
            None,
        )
    }
}

/// Registers the window class. Call once at startup.
///
/// # Errors
///
/// When the class can't be registered.
pub fn register_class(instance: HINSTANCE) -> windows::core::Result<()> {
    let class = WNDCLASSEXW {
        cbSize: struct_size::<WNDCLASSEXW>(),
        lpfnWndProc: Some(layer_proc),
        hInstance: instance,
        lpszClassName: LAYER_CLASS,
        ..Default::default()
    };
    // SAFETY: `class` is fully initialised and its strings are constants.
    if unsafe { RegisterClassExW(&raw const class) } == 0 {
        return Err(windows::core::Error::from_thread());
    }
    Ok(())
}

/// Window procedure: clicks fall through to the desktop; frames paint
/// themselves with the key; the rest keep what they were given.
unsafe extern "system" fn layer_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_NCHITTEST => {
            return LRESULT(HTTRANSPARENT.try_into().unwrap_or(-1));
        }
        WM_ERASEBKGND => return LRESULT(1),
        WM_PAINT => {
            paint_key(hwnd);
            return LRESULT(0);
        }
        _ => {}
    }
    // SAFETY: forwards the message unchanged, as Win32 requires.
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}

/// Handles `WM_PAINT`: a frame fills what's asked with the key; other
/// windows just mark it painted.
fn paint_key(hwnd: HWND) {
    let mut ps = PAINTSTRUCT::default();
    // SAFETY: called from the window's WM_PAINT with a valid
    // PAINTSTRUCT, and paired with EndPaint below. The stock brush
    // needs no cleanup; failures only skip a fill.
    unsafe {
        let hdc = BeginPaint(hwnd, &raw mut ps);
        let fills = GetWindowLongPtrW(hwnd, GWLP_USERDATA) == FILLS_WITH_KEY;
        if fills && !hdc.is_invalid() {
            let brush = HBRUSH(GetStockObject(BLACK_BRUSH).0);
            let _ = FillRect(hdc, &raw const ps.rcPaint, brush);
        }
        let _ = EndPaint(hwnd, &raw const ps);
    }
}
