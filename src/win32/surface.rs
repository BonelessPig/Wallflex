//! A surface: one window per monitor, between the wallpaper and the
//! desktop icons.

use windows::Win32::Foundation::{
    COLORREF, HINSTANCE, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM,
};
use windows::Win32::Graphics::Gdi::{
    ClientToScreen, InvalidateRect, ScreenToClient,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, GetParent, HTTRANSPARENT,
    HWND_TOP, IsWindow, LWA_ALPHA, RegisterClassExW, SW_SHOWNA, SWP_NOACTIVATE,
    SWP_NOMOVE, SWP_NOSIZE, SetLayeredWindowAttributes, SetWindowPos,
    ShowWindow, WINDOW_EX_STYLE, WM_DPICHANGED_AFTERPARENT, WM_ERASEBKGND,
    WM_NCHITTEST, WM_PAINT, WNDCLASSEXW, WS_CHILD, WS_CLIPSIBLINGS,
    WS_EX_LAYERED, WS_EX_NOACTIVATE,
};
use windows::core::{PCWSTR, w};

use super::app;
use super::desktop::{DesktopHost, Layout};
use super::monitors::Monitor;
use super::paint::Painter;
use super::struct_size;
use crate::color::Rgb;
use crate::config::Layout as ConfigLayout;
use crate::geometry::{Point, Rect};
use crate::widgets::{Action, Dashboard, Widget};

/// Window class for surfaces.
const SURFACE_CLASS: PCWSTR = w!("WallflexSurface");
/// Fully opaque, for the layered-window alpha.
const OPAQUE: u8 = 255;

/// One monitor's window and the painter that paints it.
pub struct Surface {
    /// The window, destroyed on drop.
    hwnd: HWND,
    /// Paints the window.
    painter: Painter,
    /// The monitor it covers, in desktop pixels.
    monitor: crate::geometry::Rect,
    /// Hidden behind a full-screen window: its buffers are freed and it
    /// ignores repaints until [`Surface::wake`].
    resting: bool,
}

impl Surface {
    /// Whether a full-screen window covering `window` (desktop pixels)
    /// hides this monitor's visualizer.
    pub fn visualizer_hidden_by(&self, window: crate::geometry::Rect) -> bool {
        self.painter.shows(crate::widgets::Widget::Visualizer)
            && window.covers(self.monitor)
    }

    /// Whether this monitor shows the visualizer.
    pub fn shows_visualizer(&self) -> bool {
        self.painter.shows(crate::widgets::Widget::Visualizer)
    }

    /// The monitor it covers, in desktop pixels.
    pub fn monitor(&self) -> Rect {
        self.monitor
    }

    /// Whether this monitor's layout places any widgets.
    pub fn has_widgets(&self) -> bool {
        self.painter.has_widgets()
    }

    /// Whether it's resting behind a full-screen window.
    pub fn is_resting(&self) -> bool {
        self.resting
    }

    /// Stops painting and frees the widget buffers, while a full-screen
    /// window hides the monitor. What's on screen stays as it was.
    pub fn rest(&mut self) {
        self.resting = true;
        self.painter.release_buffers();
    }

    /// Starts painting again and redraws everything.
    pub fn wake(&mut self) {
        self.resting = false;
        self.invalidate();
    }

    /// The surface's window.
    pub fn hwnd(&self) -> HWND {
        self.hwnd
    }

    /// Whether the window still exists inside `host`. Explorer
    /// restarting destroys it.
    pub fn is_attached(&self, host: &DesktopHost) -> bool {
        // SAFETY: both calls only read window state.
        unsafe {
            IsWindow(Some(self.hwnd)).as_bool()
                && GetParent(self.hwnd).ok() == Some(host.parent)
        }
    }

    /// Moves the window back just below the icons.
    ///
    /// # Errors
    ///
    /// When Windows refuses the z-order change.
    pub fn place_below_icons(
        &self,
        host: &DesktopHost,
    ) -> windows::core::Result<()> {
        // In the raised layout the icons share our parent, so go directly
        // behind them; in the classic one we're alone in the layer.
        let after = match host.layout {
            Layout::Raised => host.icons,
            Layout::Classic => HWND_TOP,
        };
        // SAFETY: plain handles and flags; size and position are kept.
        unsafe {
            SetWindowPos(
                self.hwnd,
                Some(after),
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
            )
        }
    }

    /// Asks Windows to repaint the whole window.
    pub fn invalidate(&self) {
        if self.resting {
            return;
        }
        // SAFETY: plain handle; failure only skips one repaint.
        let _ = unsafe { InvalidateRect(Some(self.hwnd), None, false) };
    }

    /// Asks Windows to repaint just the widgets.
    pub fn invalidate_widgets(&self, dashboard: &Dashboard) {
        if self.resting {
            return;
        }
        for rect in self.painter.dirty_rects(dashboard) {
            // SAFETY: plain handle and rectangle; failure only skips one
            // repaint.
            let _ = unsafe {
                InvalidateRect(Some(self.hwnd), Some(&raw const rect), false)
            };
        }
    }

    /// Asks Windows to repaint every `widget` on this monitor (and where
    /// it was, if it moved or changed size).
    pub fn invalidate_widget(&self, dashboard: &Dashboard, widget: Widget) {
        self.invalidate_rects(&self.painter.widget_rects(dashboard, widget));
    }

    /// Asks Windows to repaint just the parts of the widgets that are
    /// gliding (bars and rings).
    pub fn invalidate_moving(&self, dashboard: &Dashboard) {
        self.invalidate_rects(&self.painter.moving_rects(dashboard));
    }

    /// Asks Windows to repaint just what a quick reading changed (the
    /// CPU and network).
    pub fn invalidate_quick(&self, dashboard: &Dashboard) {
        self.invalidate_rects(&self.painter.quick_rects(dashboard));
    }

    /// Asks Windows to repaint the widgets with hover highlights.
    pub fn invalidate_hoverable(&self) {
        let rects: Vec<RECT> = self
            .painter
            .hover_rects()
            .into_iter()
            .map(super::to_win32_rect)
            .collect();
        self.invalidate_rects(&rects);
    }

    /// Where the widgets with hover highlights are, in desktop pixels.
    pub fn hover_zones(&self) -> Vec<Rect> {
        let mut origin = POINT::default();
        // SAFETY: `origin` is a valid place for the call to update.
        if !unsafe { ClientToScreen(self.hwnd, &raw mut origin) }.as_bool() {
            return Vec::new();
        }
        let shift = Point {
            x: origin.x.saturating_neg(),
            y: origin.y.saturating_neg(),
        };
        self.painter
            .hover_rects()
            .into_iter()
            .map(|r| r.relative_to(shift))
            .collect()
    }

    /// Asks Windows to repaint the visualizer.
    pub fn invalidate_visualizer(&self, dashboard: &Dashboard) {
        self.invalidate_rects(&self.painter.visualizer_rects(dashboard));
    }

    /// Asks Windows to repaint `rects` (client pixels).
    fn invalidate_rects(&self, rects: &[RECT]) {
        if self.resting {
            return;
        }
        for rect in rects {
            // SAFETY: plain handle and rectangle; failure only skips one
            // frame.
            let _ = unsafe {
                InvalidateRect(Some(self.hwnd), Some(&raw const *rect), false)
            };
        }
    }

    /// What a click at `screen` (desktop pixels) would do, if anything.
    pub fn hit_test(&self, screen: POINT) -> Option<Action> {
        let mut point = screen;
        // SAFETY: `point` is a valid place for the call to update.
        if !unsafe { ScreenToClient(self.hwnd, &raw mut point) }.as_bool() {
            return None;
        }
        self.painter.hit_test((point.x, point.y))
    }

    /// Handles `WM_PAINT`. Returns whether it painted: a resting
    /// surface doesn't, and leaves the message to the default handler.
    pub fn paint(&mut self, dashboard: &Dashboard) -> bool {
        if self.resting {
            return false;
        }
        self.painter.paint(self.hwnd, dashboard);
        true
    }
}

impl Drop for Surface {
    fn drop(&mut self) {
        // SAFETY: the window belongs to this object. It may already be
        // gone if Explorer restarted, so failure is expected and ignored.
        let _ = unsafe { DestroyWindow(self.hwnd) };
    }
}

/// Registers the surface window class. Call once at startup.
///
/// # Errors
///
/// When the class can't be registered.
pub fn register_class(instance: HINSTANCE) -> windows::core::Result<()> {
    let class = WNDCLASSEXW {
        cbSize: struct_size::<WNDCLASSEXW>(),
        lpfnWndProc: Some(surface_proc),
        hInstance: instance,
        lpszClassName: SURFACE_CLASS,
        ..Default::default()
    };
    // SAFETY: `class` is fully initialised and its strings are constants.
    if unsafe { RegisterClassExW(&raw const class) } == 0 {
        return Err(windows::core::Error::from_thread());
    }
    Ok(())
}

/// Creates a surface covering `monitor` (desktop coordinates) inside
/// `host`, just below the icons.
///
/// # Errors
///
/// When the window or its painter can't be created or placed.
pub fn create(
    host: &DesktopHost,
    monitor: &Monitor,
    instance: HINSTANCE,
    background: Rgb,
    layout: Option<&ConfigLayout>,
) -> windows::core::Result<Surface> {
    // The painter works in the surface's own coordinates: (0, 0) is the
    // monitor's top-left.
    let corner = Point {
        x: monitor.rect.left,
        y: monitor.rect.top,
    };
    let bounds = monitor.rect.relative_to(corner);
    let work = monitor.work.relative_to(corner);
    let painter = Painter::new(background, bounds, work, layout)?;

    let mut origin = POINT::default();
    // SAFETY: `origin` is a valid place for the call to write to.
    if !unsafe { ClientToScreen(host.parent, &raw mut origin) }.as_bool() {
        return Err(windows::core::Error::from_thread());
    }
    let area = monitor.rect.relative_to(Point {
        x: origin.x,
        y: origin.y,
    });

    // The raised layout needs a layered window to composite between
    // Explorer's own children (this is also why the manifest declares
    // Windows 8+).
    let layered = host.layout == Layout::Raised;
    let ex_style = if layered {
        WS_EX_NOACTIVATE | WS_EX_LAYERED
    } else {
        WS_EX_NOACTIVATE | WINDOW_EX_STYLE::default()
    };

    // SAFETY: the class is registered, the strings are constants and the
    // parent is a live window found just before.
    let hwnd = unsafe {
        CreateWindowExW(
            ex_style,
            SURFACE_CLASS,
            w!("Wallflex"),
            WS_CHILD | WS_CLIPSIBLINGS,
            area.left,
            area.top,
            area.width(),
            area.height(),
            Some(host.parent),
            None,
            Some(instance),
            None,
        )
    }?;
    // From here on, dropping `surface` destroys the window.
    let surface = Surface {
        hwnd,
        painter,
        monitor: monitor.rect,
        resting: false,
    };

    if layered {
        // SAFETY: our own layered window; plain values.
        unsafe {
            SetLayeredWindowAttributes(hwnd, COLORREF(0), OPAQUE, LWA_ALPHA)
        }?;
    }
    surface.place_below_icons(host)?;
    // SAFETY: our own window. The result is the previous visibility, not
    // an error, so it's deliberately ignored.
    let _ = unsafe { ShowWindow(hwnd, SW_SHOWNA) };
    Ok(surface)
}

/// Window procedure for surfaces.
unsafe extern "system" fn surface_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_PAINT => {
            if app::paint_surface(hwnd) {
                return LRESULT(0);
            }
        }
        // WM_PAINT covers the whole window; skipping the erase avoids a
        // flash of the class background.
        WM_ERASEBKGND => return LRESULT(1),
        // Moved to a monitor with different scaling: redo the panel.
        WM_DPICHANGED_AFTERPARENT => {
            // SAFETY: plain handle; failure only skips one repaint.
            let _ = unsafe { InvalidateRect(Some(hwnd), None, false) };
            return LRESULT(0);
        }
        // Let clicks fall through to the desktop.
        WM_NCHITTEST => return LRESULT(HTTRANSPARENT.try_into().unwrap_or(-1)),
        _ => {}
    }
    // SAFETY: forwards the message unchanged, as Win32 requires.
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}
