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
use super::layer::{LayerWindow, PictureWindow};
use super::monitors::Monitor;
use super::paint::{Painter, Place};
use super::struct_size;
use super::wallpaper::Picture;
use crate::backdrop::{self, Backdrop, Background, Mapping, Wallpaper};
use crate::color::Rgb;
use crate::config::Layout as ConfigLayout;
use crate::geometry::{Point, Rect};
use crate::log;
use crate::widgets::{Action, Dashboard};

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
    /// The picture behind the widgets, in a window of its own.
    picture: Option<PictureWindow>,
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

    /// Whether `hwnd` is one of this surface's windows: its own, its
    /// picture's or a widget's.
    pub fn owns(&self, hwnd: HWND) -> bool {
        self.windows().any(|own| own == hwnd)
    }

    /// The surface's windows, back to front.
    fn windows(&self) -> impl Iterator<Item = HWND> {
        std::iter::once(self.hwnd)
            .chain(self.picture.as_ref().map(PictureWindow::hwnd))
            .chain(self.painter.layers().map(LayerWindow::hwnd))
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

    /// Moves the window back just below the icons, with its picture and
    /// widgets' windows (if any) in front of it.
    ///
    /// # Errors
    ///
    /// When Windows refuses the z-order change.
    pub fn place_below_icons(
        &self,
        host: &DesktopHost,
    ) -> windows::core::Result<()> {
        // In the raised layout the icons share our parent, so go directly
        // behind them; in the classic one we're alone in the layer. Each
        // window placed goes in front of the ones placed before it.
        let after = match host.layout {
            Layout::Raised => host.icons,
            Layout::Classic => HWND_TOP,
        };
        for hwnd in self.windows() {
            // SAFETY: plain handles and flags; size and position are kept.
            unsafe {
                SetWindowPos(
                    hwnd,
                    Some(after),
                    0,
                    0,
                    0,
                    0,
                    SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
                )
            }?;
        }
        Ok(())
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
/// `host`, just below the icons, showing `backdrop` behind `layout`'s
/// widgets.
///
/// # Errors
///
/// When the windows or the painter can't be created or placed.
pub fn create(
    host: &DesktopHost,
    monitor: &Monitor,
    instance: HINSTANCE,
    backdrop: &Backdrop,
    layout: Option<&ConfigLayout>,
) -> windows::core::Result<Surface> {
    let mut origin = POINT::default();
    // SAFETY: `origin` is a valid place for the call to write to.
    if !unsafe { ClientToScreen(host.parent, &raw mut origin) }.as_bool() {
        return Err(windows::core::Error::from_thread());
    }
    let area = monitor.rect.relative_to(Point {
        x: origin.x,
        y: origin.y,
    });

    // The painter works in the surface's own coordinates: (0, 0) is the
    // monitor's top-left.
    let corner = Point {
        x: monitor.rect.left,
        y: monitor.rect.top,
    };
    let place = Place {
        bounds: monitor.rect.relative_to(corner),
        work: monitor.work.relative_to(corner),
        origin: Point {
            x: area.left,
            y: area.top,
        },
    };
    let painter = Painter::new(backdrop, place, layout)?;

    // The raised layout needs a layered window to composite between
    // Explorer's own children (this is also why the manifest declares
    // Windows 8+), and so does a background that lets the wallpaper
    // through.
    let layered = host.layout == Layout::Raised || backdrop.layered();
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
    let mut surface = Surface {
        hwnd,
        painter,
        picture: None,
        monitor: monitor.rect,
        resting: false,
    };

    if layered {
        // A transparent surface is fully see-through (its children would
        // be too, which is why the picture and widgets are its siblings).
        let alpha = match backdrop.background {
            Background::Transparent => 0,
            Background::Color(_) => OPAQUE,
        };
        // SAFETY: our own layered window; plain values.
        unsafe {
            SetLayeredWindowAttributes(hwnd, COLORREF(0), alpha, LWA_ALPHA)
        }?;
    }
    // The picture file stays open until it's painted, below.
    let mut pending = None;
    if backdrop.layered() {
        if let Some(image) = &backdrop.image {
            match prepare_picture((host.parent, instance), image, area) {
                Ok((window, picture, mapping)) => {
                    surface.picture = Some(window);
                    pending = Some((picture, mapping));
                }
                Err(error) => {
                    log!("can't show {}: {error}", image.path.display());
                }
            }
        }
        let layers = (0..surface.painter.widget_count())
            .map(|_| LayerWindow::create(host.parent, instance))
            .collect::<windows::core::Result<Vec<_>>>()?;
        surface.painter.attach_layers(layers);
    }
    surface.place_below_icons(host)?;
    // SAFETY: our own window. The result is the previous visibility, not
    // an error, so it's deliberately ignored.
    let _ = unsafe { ShowWindow(hwnd, SW_SHOWNA) };
    if let (Some(window), Some((picture, mapping))) =
        (&surface.picture, pending)
    {
        let matte = match backdrop.background {
            Background::Color(color) => color,
            Background::Transparent => Rgb::BLACK,
        };
        if let Err(error) = window.show(|dc| picture.paint(dc, mapping, matte))
        {
            log!("can't draw the picture: {error}");
        }
    }
    Ok(surface)
}

/// Opens `image` and makes a (hidden, unpainted) window for it, fitted
/// to a monitor at `area` (parent client pixels). Returns the window,
/// the open picture and where it goes in the window.
fn prepare_picture(
    (parent, instance): (HWND, HINSTANCE),
    image: &Wallpaper,
    area: Rect,
) -> Result<(PictureWindow, Picture, Mapping), String> {
    let picture = Picture::open(&image.path)?;
    let monitor = (area.width(), area.height());
    let mapping = backdrop::map(image.fit, picture.size(), monitor)
        .ok_or("the picture is empty")?;
    let corner = Point {
        x: mapping.dest.left,
        y: mapping.dest.top,
    };
    // The window covers just the picture, so it draws at (0, 0).
    let local = Mapping {
        dest: mapping.dest.relative_to(corner),
        ..mapping
    };
    let monitor_corner = Point {
        x: area.left,
        y: area.top,
    };
    let rect = mapping.dest.moved_by(monitor_corner);
    let window = PictureWindow::create(parent, instance, rect)?;
    Ok((window, picture, local))
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
