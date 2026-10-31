//! The reader's window: borderless, with rounded corners and the system
//! shadow, and no taskbar button. Floating, it sits in the corner of the
//! work area nearest the book icon and hides when it loses focus.
//! Pinned, it's an app bar docked full height to the nearer side, so
//! maximised windows leave room for it, and it stays until closed.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

use webview2_com::Microsoft::Web::WebView2::Win32::COREWEBVIEW2_COLOR;
use windows::Win32::Foundation::{
    COLORREF, ERROR_SUCCESS, HINSTANCE, HWND, LPARAM, LRESULT, POINT, RECT,
    WPARAM,
};
use windows::Win32::Graphics::Dwm::{
    DWMWA_BORDER_COLOR, DWMWA_USE_IMMERSIVE_DARK_MODE,
    DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND, DWMWINDOWATTRIBUTE,
    DwmSetWindowAttribute,
};
use windows::Win32::Graphics::Gdi::{
    CreateSolidBrush, GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO,
    MonitorFromPoint,
};
use windows::Win32::System::Registry::{
    HKEY_CURRENT_USER, RRF_RT_REG_DWORD, RegGetValueW,
};
use windows::Win32::System::Threading::{
    AttachThreadInput, GetCurrentThreadId,
};
use windows::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
use windows::Win32::UI::Shell::{
    ABE_LEFT, ABE_RIGHT, ABM_NEW, ABM_QUERYPOS, ABM_REMOVE, ABM_SETPOS,
    ABN_FULLSCREENAPP, ABN_POSCHANGED, APPBARDATA, SHAppBarMessage,
};
use windows::Win32::UI::WindowsAndMessaging::{
    BringWindowToTop, CreateWindowExW, DestroyWindow, GetClientRect,
    GetForegroundWindow, GetWindowThreadProcessId, HWND_BOTTOM, HWND_NOTOPMOST,
    HWND_TOPMOST, IDC_ARROW, KillTimer, LoadCursorW, PostMessageW,
    PostQuitMessage, RegisterClassW, RegisterWindowMessageW, SW_HIDE,
    SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_SHOWWINDOW,
    SetForegroundWindow, SetTimer, SetWindowPos, ShowWindow, WA_INACTIVE,
    WM_ACTIVATE, WM_APP, WM_CLOSE, WM_DESTROY, WM_DISPLAYCHANGE, WM_DPICHANGED,
    WM_NCCALCSIZE, WM_SIZE, WM_TIMER, WNDCLASSW, WNDPROC, WS_CLIPCHILDREN,
    WS_EX_TOOLWINDOW, WS_POPUP, WS_THICKFRAME,
};
use windows::core::{BOOL, HSTRING, PCWSTR, w};

use super::webview::WebView;
use super::{Event, Launch, with_reader};
use crate::messages::{self, Request};
use crate::place::{self, Edge, MARGIN, PANE_WIDTH, PANEL_SIZE, Rect, Scale};
use crate::saved::Saved;

/// How long the window may stay hidden before the reader exits, in
/// milliseconds.
pub const EXIT_AFTER_MS: u32 = 120_000;
/// The smallest and largest zoom remembered, on top of the usual size.
const MIN_ZOOM: f64 = 0.25;
/// See [`MIN_ZOOM`].
const MAX_ZOOM: f64 = 5.0;
/// Zoom changes smaller than this aren't worth saving.
const ZOOM_STEP: f64 = 0.001;
/// The timer that counts that down.
const EXIT_TIMER: usize = 1;
/// A book-icon click this soon after the window hid itself (because the
/// same click took the focus away) leaves it hidden.
const TOGGLE_GRACE: Duration = Duration::from_millis(800);
/// The app bar's notifications.
const WM_APPBAR: u32 = WM_APP + 1;
/// Posted when the window loses the focus.
const WM_DEACTIVATED: u32 = WM_APP + 2;
/// Posted when the window gets the focus.
const WM_ACTIVATED: u32 = WM_APP + 3;
/// The low 16 bits of a message parameter.
const LOW_WORD: usize = 0xFFFF;
/// DPI at 100% scaling.
const BASE_DPI: u32 = 96;
/// `DWMWA_COLOR_NONE`: no window border.
const NO_BORDER: COLORREF = COLORREF(0xFFFF_FFFE);
/// Where Windows keeps the app colour mode.
const THEME_KEY: PCWSTR =
    w!("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize");

/// `TaskbarCreated`, sent when Explorer restarts (app bars are lost).
static TASKBAR_CREATED: AtomicU32 = AtomicU32::new(0);

/// A monitor, as far as placing the window cares.
#[derive(Clone, Copy)]
struct Screen {
    /// The whole monitor.
    monitor: Rect,
    /// The part the taskbar (and app bars) leave free.
    work: Rect,
    /// Its scaling, in DPI.
    dpi: u32,
}

/// The window, its browser and what the reader remembers.
pub struct Reader {
    /// The window.
    hwnd: HWND,
    /// The browser filling it.
    view: WebView,
    /// The guides folder.
    folder: PathBuf,
    /// Where `reader.json` is.
    saved_path: PathBuf,
    /// What's remembered.
    saved: Saved,
    /// Where the book icon was last clicked.
    at: (i32, i32),
    /// Whether the window is showing.
    shown: bool,
    /// Whether it's registered as an app bar.
    appbar: bool,
    /// When it last hid itself on losing the focus.
    hidden_at: Option<Instant>,
    /// The page's zoom for this monitor, before the user's own.
    base_zoom: f64,
}

impl Reader {
    /// Creates the (hidden) window and its browser.
    ///
    /// # Errors
    ///
    /// Why the window or the browser couldn't be made.
    pub fn create(
        instance: HINSTANCE,
        class: PCWSTR,
        window_proc: WNDPROC,
        launch: &Launch,
    ) -> Result<Self, String> {
        let saved = Saved::load(&launch.saved);
        let ([r, g, b], dark) = saved.style.background(!light_theme());
        let hwnd = create_window(instance, class, window_proc, [r, g, b])?;
        style_window(hwnd, dark);
        let background = COREWEBVIEW2_COLOR {
            A: 255,
            R: r,
            G: g,
            B: b,
        };
        let view =
            WebView::create(hwnd, &launch.data, &launch.folder, background)?;
        Ok(Self {
            hwnd,
            view,
            folder: launch.folder.clone(),
            saved_path: launch.saved.clone(),
            saved,
            at: (0, 0),
            shown: false,
            appbar: false,
            hidden_at: None,
            base_zoom: 1.0,
        })
    }

    /// Closes the browser, before the reader exits.
    pub fn close(self) {
        self.view.close();
    }

    /// The book icon was clicked at `at`: shows the window there, or
    /// hides it if it's showing.
    pub fn toggle(&mut self, at: (i32, i32)) {
        if self.shown {
            self.hide(false);
        } else if self
            .hidden_at
            .take()
            .is_some_and(|hidden| hidden.elapsed() < TOGGLE_GRACE)
        {
            // The click that hid the window: it stays hidden.
        } else {
            self.show(at);
        }
    }

    /// Shows the window next to `at` and brings it to the front.
    pub fn show(&mut self, at: (i32, i32)) {
        // SAFETY: plain values; a timer that isn't running is fine.
        let _ = unsafe { KillTimer(Some(self.hwnd), EXIT_TIMER) };
        self.at = at;
        self.hidden_at = None;
        self.place();
        self.view.set_visible(true);
        self.shown = true;
        bring_to_front(self.hwnd);
        self.send_list();
    }

    /// Hides the window (`deactivated`: because it lost the focus) and
    /// starts the countdown to exiting.
    fn hide(&mut self, deactivated: bool) {
        if !self.shown {
            return;
        }
        self.shown = false;
        self.remove_appbar();
        // SAFETY: plain values; the result only says if it was visible.
        let _ = unsafe { ShowWindow(self.hwnd, SW_HIDE) };
        self.view.set_visible(false);
        self.hidden_at = deactivated.then(Instant::now);
        // SAFETY: plain values. If the timer can't start, the reader
        // just stays until it's closed some other way.
        let _ = unsafe {
            SetTimer(Some(self.hwnd), EXIT_TIMER, EXIT_AFTER_MS, None)
        };
    }

    /// Moves the window where it belongs: floating in the corner nearest
    /// the last click, or docked if pinned.
    fn place(&mut self) {
        let Some(screen) = screen_at(self.at) else {
            return;
        };
        let scale = Scale::new(screen.dpi, screen.monitor.height());
        self.base_zoom = scale.zoom(screen.dpi);
        self.view.set_zoom(self.base_zoom * self.saved.zoom);
        let (rect, order) = if self.saved.pinned {
            let at = self.at;
            let edge = *self
                .saved
                .side
                .get_or_insert_with(|| place::edge_for(screen.monitor, at));
            let width = scale.apply(PANE_WIDTH);
            (self.dock(screen, edge, width), HWND_TOPMOST)
        } else {
            let had_appbar = self.appbar;
            self.remove_appbar();
            // Without the app bar the work area is wider again.
            let screen = if had_appbar {
                screen_at(self.at).unwrap_or(screen)
            } else {
                screen
            };
            let size = (scale.apply(PANEL_SIZE.0), scale.apply(PANEL_SIZE.1));
            let margin = scale.apply(MARGIN);
            (
                place::floating(screen.work, self.at, size, margin),
                HWND_NOTOPMOST,
            )
        };
        // SAFETY: plain values on our own window. If it fails, the window
        // stays where it was.
        let _ = unsafe {
            SetWindowPos(
                self.hwnd,
                Some(order),
                rect.left,
                rect.top,
                rect.width(),
                rect.height(),
                SWP_SHOWWINDOW,
            )
        };
        self.fit();
    }

    /// Registers the window as an app bar against `edge` of `screen`,
    /// `width` wide, and returns where the window goes. Without an app
    /// bar it still docks, but windows may cover it.
    fn dock(&mut self, screen: Screen, edge: Edge, width: i32) -> Rect {
        let mut data = self.appbar_data();
        if !self.appbar {
            // SAFETY: `data` is a valid APPBARDATA for our window.
            self.appbar =
                unsafe { SHAppBarMessage(ABM_NEW, &raw mut data) } != 0;
        }
        let pane = place::docked(screen.monitor, edge, width);
        let height = Rect {
            top: screen.work.top,
            bottom: screen.work.bottom,
            ..pane
        };
        if !self.appbar {
            return height;
        }
        let width = pane.width();
        data.uEdge = match edge {
            Edge::Left => ABE_LEFT,
            Edge::Right => ABE_RIGHT,
        };
        data.rc = to_win32(pane);
        // SAFETY: as above. Windows moves `rc` out of other app bars'
        // way; the pane then keeps its width from the outer edge.
        unsafe {
            SHAppBarMessage(ABM_QUERYPOS, &raw mut data);
            match edge {
                Edge::Left => {
                    data.rc.right = data.rc.left.saturating_add(width);
                }
                Edge::Right => {
                    data.rc.left = data.rc.right.saturating_sub(width);
                }
            }
            SHAppBarMessage(ABM_SETPOS, &raw mut data);
        }
        Rect {
            left: data.rc.left,
            right: data.rc.right,
            ..height
        }
    }

    /// Stops being an app bar, giving the space back.
    fn remove_appbar(&mut self) {
        if self.appbar {
            let mut data = self.appbar_data();
            // SAFETY: `data` is a valid APPBARDATA for our window.
            unsafe { SHAppBarMessage(ABM_REMOVE, &raw mut data) };
            self.appbar = false;
        }
    }

    /// The app bar details every call starts from.
    fn appbar_data(&self) -> APPBARDATA {
        APPBARDATA {
            cbSize: struct_size::<APPBARDATA>(),
            hWnd: self.hwnd,
            uCallbackMessage: WM_APPBAR,
            ..APPBARDATA::default()
        }
    }

    /// Handles an app bar notification.
    fn on_appbar(&mut self, wparam: WPARAM, lparam: LPARAM) {
        let code = u32::try_from(wparam.0).unwrap_or_default();
        if code == ABN_POSCHANGED && self.shown && self.saved.pinned {
            self.place();
        } else if code == ABN_FULLSCREENAPP {
            // Step behind a full-screen app, and back in front after.
            let order = if lparam.0 == 0 {
                HWND_TOPMOST
            } else {
                HWND_BOTTOM
            };
            // SAFETY: plain values on our own window.
            let _ = unsafe {
                SetWindowPos(
                    self.hwnd,
                    Some(order),
                    0,
                    0,
                    0,
                    0,
                    SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
                )
            };
        }
    }

    /// Fits the browser to the window.
    fn fit(&self) {
        let mut rect = RECT::default();
        // SAFETY: `rect` is a valid place for the call to write.
        if unsafe { GetClientRect(self.hwnd, &raw mut rect) }.is_ok() {
            self.view.fit(rect.right, rect.bottom);
        }
    }

    /// Acts on what the page or browser reported.
    pub fn on_event(&mut self, event: Event) {
        match event {
            Event::Request(request) => self.on_request(request),
            Event::Open(name) => self.open(&name),
            // The page closes its guide list first, or asks to close.
            Event::Escape => self.view.post(&messages::escape()),
            Event::Zoomed(zoom) => {
                let own = (zoom / self.base_zoom).clamp(MIN_ZOOM, MAX_ZOOM);
                if (own - self.saved.zoom).abs() > ZOOM_STEP {
                    self.saved.zoom = own;
                    self.remember();
                }
            }
        }
    }

    /// Answers the page.
    fn on_request(&mut self, request: Request) {
        match request {
            Request::Ready | Request::List => self.send_list(),
            Request::Open(name) => self.open(&name),
            Request::Pin(edge) => {
                if self.saved.pinned && self.saved.side == Some(edge) {
                    self.saved.pinned = false;
                } else {
                    if self.saved.pinned {
                        // Changing sides: register again on the new one.
                        self.remove_appbar();
                    }
                    self.saved.pinned = true;
                    self.saved.side = Some(edge);
                }
                self.remember();
                self.place();
                bring_to_front(self.hwnd);
                self.view.post(&messages::pinned(
                    self.saved.pinned,
                    self.saved.side,
                ));
            }
            Request::Close => self.hide(false),
            Request::Style(style) => {
                self.saved.style = style.checked();
                self.remember();
                let (_, dark) = self.saved.style.background(!light_theme());
                style_window(self.hwnd, dark);
            }
            Request::Folder => {
                // It's made at startup; this only matters if it was
                // deleted since, and Explorer says so if it's missing.
                let _ = std::fs::create_dir_all(&self.folder);
                super::shell_open(&HSTRING::from(self.folder.as_os_str()));
            }
        }
    }

    /// Sends the page the guides.
    fn send_list(&self) {
        self.view.post(&messages::list(&self.folder, &self.saved));
    }

    /// Shows guide `name`, remembering it if it worked.
    fn open(&mut self, name: &str) {
        let (answer, ok) = messages::show(&self.folder, name);
        self.view.post(&answer);
        if ok && self.saved.last.as_deref() != Some(name) {
            self.saved.last = Some(name.to_owned());
            self.remember();
        }
    }

    /// Saves what's remembered.
    fn remember(&self) {
        // Losing the pin or the last guide is harmless; the reader works
        // the same without them.
        let _ = self.saved.save(&self.saved_path);
    }
}

/// Handles the window's own messages; `None` passes one to the default
/// handler. Changes the reader only through [`with_reader`], since some
/// arrive while it's busy (moving the window sends `WM_SIZE`).
pub fn handle_message(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> Option<LRESULT> {
    match msg {
        // No frame: the client area is the whole window.
        WM_NCCALCSIZE if wparam.0 != 0 => Some(LRESULT(0)),
        WM_ACTIVATE => {
            let inactive = wparam.0 & LOW_WORD
                == usize::try_from(WA_INACTIVE).unwrap_or_default();
            let note = if inactive {
                WM_DEACTIVATED
            } else {
                WM_ACTIVATED
            };
            // SAFETY: posting to our own window; plain values. A lost
            // note only skips hiding or focusing once.
            let _ =
                unsafe { PostMessageW(Some(hwnd), note, WPARAM(0), LPARAM(0)) };
            None
        }
        WM_DEACTIVATED => {
            // Showing the window can take the focus away and back; only
            // a focus that's still elsewhere now counts.
            // SAFETY: plain call.
            let focused = unsafe { GetForegroundWindow() } == hwnd;
            with_reader(|reader| {
                if !focused && !reader.saved.pinned {
                    reader.hide(true);
                }
            });
            Some(LRESULT(0))
        }
        WM_ACTIVATED => {
            // Focusing the page while hidden would activate the window.
            with_reader(|reader| {
                if reader.shown {
                    reader.view.focus();
                }
            });
            Some(LRESULT(0))
        }
        WM_SIZE => {
            with_reader(|reader| reader.fit());
            Some(LRESULT(0))
        }
        WM_TIMER if wparam.0 == EXIT_TIMER => {
            // SAFETY: our own window; WM_DESTROY then ends the loop.
            let _ = unsafe { DestroyWindow(hwnd) };
            Some(LRESULT(0))
        }
        WM_CLOSE => {
            with_reader(|reader| reader.hide(false));
            Some(LRESULT(0))
        }
        WM_APPBAR => {
            with_reader(|reader| reader.on_appbar(wparam, lparam));
            Some(LRESULT(0))
        }
        WM_DPICHANGED | WM_DISPLAYCHANGE => {
            with_reader(|reader| {
                if reader.shown {
                    reader.place();
                }
            });
            Some(LRESULT(0))
        }
        WM_DESTROY => {
            with_reader(Reader::remove_appbar);
            // SAFETY: plain value; ends the message loop.
            unsafe { PostQuitMessage(0) };
            Some(LRESULT(0))
        }
        _ if msg != 0 && msg == TASKBAR_CREATED.load(Ordering::Relaxed) => {
            // Explorer restarted and forgot the app bar.
            with_reader(|reader| {
                reader.appbar = false;
                if reader.shown {
                    reader.place();
                }
            });
            Some(LRESULT(0))
        }
        _ => None,
    }
}

/// Registers the window class and creates the hidden window, with
/// `background` (RGB) behind the browser.
fn create_window(
    instance: HINSTANCE,
    class: PCWSTR,
    window_proc: WNDPROC,
    background: [u8; 3],
) -> Result<HWND, String> {
    let [r, g, b] = background;
    // SAFETY: plain values; the brush lives as long as the class.
    let brush =
        unsafe { CreateSolidBrush(COLORREF(u32::from_le_bytes([r, g, b, 0]))) };
    let window_class = WNDCLASSW {
        lpfnWndProc: window_proc,
        hInstance: instance,
        // SAFETY: loads a stock cursor.
        hCursor: unsafe { LoadCursorW(None, IDC_ARROW) }.unwrap_or_default(),
        hbrBackground: brush,
        lpszClassName: class,
        ..WNDCLASSW::default()
    };
    // SAFETY: the class's strings are static.
    if unsafe { RegisterClassW(&raw const window_class) } == 0 {
        return Err("can't register the window class".to_owned());
    }
    // SAFETY: as above.
    TASKBAR_CREATED.store(
        unsafe { RegisterWindowMessageW(w!("TaskbarCreated")) },
        Ordering::Relaxed,
    );
    // SAFETY: the class was just registered; plain values otherwise.
    unsafe {
        CreateWindowExW(
            WS_EX_TOOLWINDOW,
            class,
            w!("Guides"),
            WS_POPUP | WS_THICKFRAME | WS_CLIPCHILDREN,
            0,
            0,
            1,
            1,
            None,
            None,
            Some(instance),
            None,
        )
    }
    .map_err(|e| e.message())
}

/// Rounded corners, no border line, and the dark frame in dark mode.
/// The frame (`WS_THICKFRAME`) is what gives the window its shadow.
fn style_window(hwnd: HWND, dark: bool) {
    set_attribute(hwnd, DWMWA_WINDOW_CORNER_PREFERENCE, &DWMWCP_ROUND);
    set_attribute(hwnd, DWMWA_BORDER_COLOR, &NO_BORDER);
    set_attribute(hwnd, DWMWA_USE_IMMERSIVE_DARK_MODE, &BOOL::from(dark));
}

/// Sets a window attribute; older Windows versions without it skip it.
fn set_attribute<T>(hwnd: HWND, attribute: DWMWINDOWATTRIBUTE, value: &T) {
    // SAFETY: `value` is the type the attribute takes and outlives the
    // call, which copies it.
    let _ = unsafe {
        DwmSetWindowAttribute(
            hwnd,
            attribute,
            std::ptr::from_ref(value).cast(),
            struct_size::<T>(),
        )
    };
}

/// Whether apps are in light mode (the default if it can't be read).
fn light_theme() -> bool {
    let mut value = 1_u32;
    let mut size = struct_size::<u32>();
    // SAFETY: `value` and `size` are valid places for the call to write,
    // and `size` says how big `value` is.
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            THEME_KEY,
            w!("AppsUseLightTheme"),
            RRF_RT_REG_DWORD,
            None,
            Some(std::ptr::from_mut(&mut value).cast()),
            Some(&raw mut size),
        )
    };
    status != ERROR_SUCCESS || value != 0
}

/// The monitor nearest `at`.
fn screen_at((x, y): (i32, i32)) -> Option<Screen> {
    // SAFETY: plain values; it always returns a monitor.
    let monitor =
        unsafe { MonitorFromPoint(POINT { x, y }, MONITOR_DEFAULTTONEAREST) };
    let mut info = MONITORINFO {
        cbSize: struct_size::<MONITORINFO>(),
        ..MONITORINFO::default()
    };
    // SAFETY: `info` is a valid MONITORINFO with its size set.
    if !unsafe { GetMonitorInfoW(monitor, &raw mut info) }.as_bool() {
        return None;
    }
    let (mut dpi, mut dpi_y) = (BASE_DPI, BASE_DPI);
    // SAFETY: both are valid places for the call to write. On failure
    // they stay at 100%.
    let _ = unsafe {
        GetDpiForMonitor(
            monitor,
            MDT_EFFECTIVE_DPI,
            &raw mut dpi,
            &raw mut dpi_y,
        )
    };
    Some(Screen {
        monitor: from_win32(info.rcMonitor),
        work: from_win32(info.rcWork),
        dpi,
    })
}

/// Brings the window to the front with the keyboard focus. A program
/// may only take the focus in some cases; when it can't, it borrows the
/// focused window's input queue for a moment, as Windows allows.
fn bring_to_front(hwnd: HWND) {
    // SAFETY: plain values throughout; each call fails harmlessly, at
    // worst leaving the window behind the focused one.
    unsafe {
        if SetForegroundWindow(hwnd).as_bool() {
            return;
        }
        let theirs = GetWindowThreadProcessId(GetForegroundWindow(), None);
        let ours = GetCurrentThreadId();
        if theirs != 0
            && theirs != ours
            && AttachThreadInput(ours, theirs, true).as_bool()
        {
            let _ = SetForegroundWindow(hwnd);
            let _ = BringWindowToTop(hwnd);
            let _ = AttachThreadInput(ours, theirs, false);
        }
    }
}

/// A Win32 rectangle as ours.
fn from_win32(rect: RECT) -> Rect {
    Rect {
        left: rect.left,
        top: rect.top,
        right: rect.right,
        bottom: rect.bottom,
    }
}

/// Ours as a Win32 rectangle.
fn to_win32(rect: Rect) -> RECT {
    RECT {
        left: rect.left,
        top: rect.top,
        right: rect.right,
        bottom: rect.bottom,
    }
}

/// The size of `T` as Win32 wants it.
fn struct_size<T>() -> u32 {
    // Win32 structs are far smaller than 4 GB; the fallback never runs.
    u32::try_from(size_of::<T>()).unwrap_or(u32::MAX)
}
