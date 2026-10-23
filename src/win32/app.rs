//! The main loop: a hidden controller window owns one surface per
//! monitor, checks on them every few seconds, and rebuilds them when
//! Explorer restarts, the displays change or the PC wakes up.

use std::cell::RefCell;
use std::fmt;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};

use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::Console::SetConsoleCtrlHandler;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW,
    FindWindowW, GW_HWNDNEXT, GetMessageW, GetWindow, IsWindow, KillTimer, MSG,
    PBT_APMRESUMEAUTOMATIC, PostMessageW, PostQuitMessage, RegisterClassExW,
    RegisterWindowMessageW, SetTimer, TranslateMessage, WM_CLOSE, WM_DESTROY,
    WM_DISPLAYCHANGE, WM_POWERBROADCAST, WM_TIMER, WNDCLASSEXW,
    WS_EX_TOOLWINDOW, WS_POPUP,
};
use windows::core::{BOOL, PCWSTR, w};

use super::desktop::{self, DesktopHost, Layout};
use super::surface::{self, Surface};
use super::{monitors, struct_size};
use crate::args::Options;

/// Window class of the hidden controller window. Also how `--quit` and
/// the single-instance check find a running copy.
const CONTROLLER_CLASS: PCWSTR = w!("WallflexController");

/// Timer that checks the surfaces are still in place.
const HEALTH_TIMER: usize = 1;
/// How often the health check runs, in milliseconds.
const HEALTH_INTERVAL_MS: u32 = 5_000;
/// One-shot timer that rebuilds the surfaces.
const REBUILD_TIMER: usize = 2;
/// Wait after a display change before rebuilding, in milliseconds, so a
/// burst of changes causes one rebuild.
const DISPLAY_SETTLE_MS: u32 = 1_000;
/// Wait after Explorer restarts before rebuilding, in milliseconds,
/// while it recreates the desktop.
const EXPLORER_SETTLE_MS: u32 = 2_000;
/// Health checks to skip after a failed attempt, so a desktop that won't
/// split is asked again every 30 seconds rather than every 5.
const RETRY_SKIP_TICKS: u32 = 5;

/// Options for this run, set once at startup.
static OPTIONS: OnceLock<Options> = OnceLock::new();
/// The controller window's handle as an address, for the Ctrl+C handler
/// (which runs on another thread). 0 when not running.
static CONTROLLER: AtomicUsize = AtomicUsize::new(0);
/// Message id Explorer broadcasts when it (re)starts.
static TASKBAR_CREATED: AtomicU32 = AtomicU32::new(0);

/// What the controller owns. Lives on the UI thread only.
#[derive(Default)]
struct State {
    /// Where the surfaces are, when they exist.
    host: Option<DesktopHost>,
    /// One per monitor.
    surfaces: Vec<Surface>,
    /// Health checks to skip before retrying after a failure.
    skip_ticks: u32,
    /// A rebuild is already scheduled, so nothing else asks for one.
    rebuild_pending: bool,
}

thread_local! {
    /// The controller's state.
    static STATE: RefCell<State> = RefCell::new(State::default());
}

/// Why Wallflex couldn't run.
#[derive(Debug)]
pub enum AppError {
    /// Another copy is already running.
    AlreadyRunning,
    /// A Win32 call failed.
    Windows(windows::core::Error),
}

impl fmt::Display for AppError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::AlreadyRunning => write!(f, "Wallflex is already running"),
            Self::Windows(error) => write!(f, "Windows error: {error}"),
        }
    }
}

impl std::error::Error for AppError {}

impl From<windows::core::Error> for AppError {
    fn from(error: windows::core::Error) -> Self {
        Self::Windows(error)
    }
}

/// Health of the surfaces, from [`check`].
enum Health {
    /// Everything in place.
    Good,
    /// Surfaces exist but something got above them.
    WrongOrder,
    /// Surfaces or the host are gone; rebuild.
    Broken,
}

/// Runs until `--quit`, Ctrl+C or the window is closed.
///
/// # Errors
///
/// [`AppError::AlreadyRunning`] if another copy is running, or the Win32
/// error that stopped startup or the message loop.
pub fn run(options: Options) -> Result<(), AppError> {
    if find_controller().is_some() {
        return Err(AppError::AlreadyRunning);
    }
    // Only fails if already set, which can't happen before this point.
    let _ = OPTIONS.set(options);

    let instance = module_instance()?;
    register_controller_class(instance)?;
    surface::register_class(instance)?;

    // SAFETY: the string is a valid null-terminated constant.
    let taskbar = unsafe { RegisterWindowMessageW(w!("TaskbarCreated")) };
    TASKBAR_CREATED.store(taskbar, Ordering::Relaxed);

    // A hidden top-level window: it never shows, but it receives the
    // broadcasts (Explorer restart, display change, resume) we act on.
    // SAFETY: the class is registered and the strings are constants.
    let controller = unsafe {
        CreateWindowExW(
            WS_EX_TOOLWINDOW,
            CONTROLLER_CLASS,
            w!("Wallflex"),
            WS_POPUP,
            0,
            0,
            0,
            0,
            None,
            None,
            Some(instance),
            None,
        )
    }?;
    CONTROLLER.store(controller.0.expose_provenance(), Ordering::Release);

    // SAFETY: the handler is a plain function that only posts a message.
    // Without a console (release builds) this fails harmlessly.
    let _ = unsafe { SetConsoleCtrlHandler(Some(on_console_ctrl), true) };

    rebuild();
    // SAFETY: our own window; no callback, so WM_TIMER is posted.
    if unsafe {
        SetTimer(Some(controller), HEALTH_TIMER, HEALTH_INTERVAL_MS, None)
    } == 0
    {
        return Err(windows::core::Error::from_thread().into());
    }

    let result = message_loop();
    CONTROLLER.store(0, Ordering::Release);
    result
}

/// Asks a running copy to quit. Returns whether one was found.
pub fn request_quit() -> bool {
    find_controller().is_some_and(post_close)
}

/// Describes the desktop windows, for `--diagnose`.
pub fn diagnose() -> String {
    desktop::describe()
}

/// Runs `action` on the surface whose window is `hwnd`. Returns whether
/// it was found (it isn't while being created or during a rebuild).
pub(super) fn with_surface(hwnd: HWND, action: fn(&mut Surface)) -> bool {
    STATE.with(|state| {
        let Ok(mut state) = state.try_borrow_mut() else {
            return false;
        };
        let found = state.surfaces.iter_mut().find(|s| s.hwnd() == hwnd);
        found.map(action).is_some()
    })
}

/// Pumps messages until `WM_QUIT`.
fn message_loop() -> Result<(), AppError> {
    let mut msg = MSG::default();
    loop {
        // SAFETY: `msg` is a valid place for the call to write to.
        let status = unsafe { GetMessageW(&raw mut msg, None, 0, 0) };
        match status.0 {
            0 => return Ok(()),
            -1 => return Err(windows::core::Error::from_thread().into()),
            _ => {
                // SAFETY: `msg` was just filled by GetMessageW. Both
                // results are informational, not errors.
                unsafe {
                    let _ = TranslateMessage(&raw const msg);
                    let _ = DispatchMessageW(&raw const msg);
                }
            }
        }
    }
}

/// This exe's module handle.
fn module_instance() -> windows::core::Result<HINSTANCE> {
    // SAFETY: `None` asks for the current exe, which is always loaded.
    let module = unsafe { GetModuleHandleW(PCWSTR::null()) }?;
    Ok(module.into())
}

/// Registers the controller window class.
fn register_controller_class(instance: HINSTANCE) -> windows::core::Result<()> {
    let class = WNDCLASSEXW {
        cbSize: struct_size::<WNDCLASSEXW>(),
        lpfnWndProc: Some(controller_proc),
        hInstance: instance,
        lpszClassName: CONTROLLER_CLASS,
        ..Default::default()
    };
    // SAFETY: `class` is fully initialised and its strings are constants.
    if unsafe { RegisterClassExW(&raw const class) } == 0 {
        return Err(windows::core::Error::from_thread());
    }
    Ok(())
}

/// A running copy's controller window, if any.
fn find_controller() -> Option<HWND> {
    // SAFETY: both strings are valid null-terminated constants.
    unsafe { FindWindowW(CONTROLLER_CLASS, PCWSTR::null()) }.ok()
}

/// Posts `WM_CLOSE` to `hwnd`. Returns whether it was posted.
fn post_close(hwnd: HWND) -> bool {
    // SAFETY: posting is safe from any thread; plain values only.
    unsafe { PostMessageW(Some(hwnd), WM_CLOSE, WPARAM(0), LPARAM(0)) }.is_ok()
}

/// Ctrl+C / console close: ask the UI thread to shut down cleanly.
unsafe extern "system" fn on_console_ctrl(_kind: u32) -> BOOL {
    let address = CONTROLLER.load(Ordering::Acquire);
    if address != 0 {
        let _posted =
            post_close(HWND(std::ptr::with_exposed_provenance_mut(address)));
    }
    true.into()
}

/// Window procedure for the controller.
unsafe extern "system" fn controller_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_TIMER => {
            on_timer(hwnd, wparam.0);
            return LRESULT(0);
        }
        WM_DISPLAYCHANGE => {
            schedule_rebuild(hwnd, DISPLAY_SETTLE_MS);
            return LRESULT(0);
        }
        WM_POWERBROADCAST => {
            if u32::try_from(wparam.0) == Ok(PBT_APMRESUMEAUTOMATIC) {
                schedule_rebuild(hwnd, DISPLAY_SETTLE_MS);
            }
            return LRESULT(1);
        }
        WM_CLOSE => {
            // SAFETY: our own window, on its own thread. On failure the
            // loop keeps running, which is the only sane fallback.
            let _ = unsafe { DestroyWindow(hwnd) };
            return LRESULT(0);
        }
        WM_DESTROY => {
            drop(take_surfaces());
            // SAFETY: plain value; ends the message loop.
            unsafe { PostQuitMessage(0) };
            return LRESULT(0);
        }
        _ => {}
    }
    let taskbar_created = TASKBAR_CREATED.load(Ordering::Relaxed);
    if taskbar_created != 0 && msg == taskbar_created {
        on_explorer_restart(hwnd);
        return LRESULT(0);
    }
    // SAFETY: forwards the message unchanged, as Win32 requires.
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}

/// Handles both timers.
fn on_timer(hwnd: HWND, timer: usize) {
    match timer {
        REBUILD_TIMER => {
            // SAFETY: our own timer; failure only means it was already
            // gone.
            let _ = unsafe { KillTimer(Some(hwnd), REBUILD_TIMER) };
            set_rebuild_pending(false);
            rebuild();
        }
        HEALTH_TIMER => on_health_tick(hwnd),
        _ => {}
    }
}

/// (Re)starts the one-shot rebuild timer. Every rebuild goes through it,
/// so requests that arrive close together cause a single rebuild.
fn schedule_rebuild(hwnd: HWND, delay_ms: u32) {
    set_rebuild_pending(true);
    // SAFETY: our own window; re-arming an existing id resets it.
    if unsafe { SetTimer(Some(hwnd), REBUILD_TIMER, delay_ms, None) } == 0 {
        // Let the next health check try again.
        set_rebuild_pending(false);
    }
}

/// Records whether a rebuild is scheduled.
fn set_rebuild_pending(pending: bool) {
    STATE.with(|state| {
        if let Ok(mut state) = state.try_borrow_mut() {
            state.rebuild_pending = pending;
        }
    });
}

/// Explorer restarted. Rebuilds unless a rebuild is already scheduled or
/// a health check has already put the surfaces back.
fn on_explorer_restart(hwnd: HWND) {
    let settled = STATE.with(|state| {
        state.try_borrow().is_ok_and(|state| {
            state.rebuild_pending
                || state.host.is_some_and(|host| {
                    matches!(check(&host, &state.surfaces), Health::Good)
                })
        })
    });
    if !settled {
        schedule_rebuild(hwnd, EXPLORER_SETTLE_MS);
    }
}

/// Checks the surfaces, then repairs them or schedules a rebuild.
fn on_health_tick(hwnd: HWND) {
    let verdict = STATE.with(|state| {
        let mut state = state.try_borrow_mut().ok()?;
        if state.rebuild_pending {
            return None;
        }
        match state.host {
            Some(host) => Some(check(&host, &state.surfaces)),
            None if state.skip_ticks > 0 => {
                state.skip_ticks = state.skip_ticks.saturating_sub(1);
                Some(Health::Good)
            }
            None => Some(Health::Broken),
        }
    });
    match verdict {
        // Waiting lets Explorer finish starting if that's what broke them.
        Some(Health::Broken) => schedule_rebuild(hwnd, EXPLORER_SETTLE_MS),
        Some(Health::WrongOrder) => restore_order(),
        Some(Health::Good) | None => {}
    }
}

/// Judges whether the surfaces are still where they should be.
fn check(host: &DesktopHost, surfaces: &[Surface]) -> Health {
    // SAFETY: IsWindow only reads window state.
    let host_alive = unsafe {
        IsWindow(Some(host.parent)).as_bool()
            && IsWindow(Some(host.icons)).as_bool()
    };
    if !host_alive
        || surfaces.is_empty()
        || surfaces.iter().any(|s| !s.is_attached(host))
    {
        return Health::Broken;
    }
    if host.layout == Layout::Raised {
        // SAFETY: GetWindow only reads window relationships.
        let below_icons = unsafe { GetWindow(host.icons, GW_HWNDNEXT) }.ok();
        if !surfaces.iter().any(|s| Some(s.hwnd()) == below_icons) {
            return Health::WrongOrder;
        }
    }
    Health::Good
}

/// Puts every surface back just below the icons.
fn restore_order() {
    STATE.with(|state| {
        let Ok(state) = state.try_borrow() else {
            return;
        };
        let Some(host) = state.host else {
            return;
        };
        for surface in &state.surfaces {
            if let Err(error) = surface.place_below_icons(&host) {
                eprintln!("wallflex: can't restore window order: {error}");
            }
        }
    });
}

/// Removes the surfaces from the state so they can be dropped without
/// the state borrowed (dropping destroys windows, which sends messages).
fn take_surfaces() -> Vec<Surface> {
    STATE.with(|state| {
        state
            .try_borrow_mut()
            .map(|mut state| {
                state.host = None;
                std::mem::take(&mut state.surfaces)
            })
            .unwrap_or_default()
    })
}

/// Destroys the surfaces and creates fresh ones on every monitor.
fn rebuild() {
    drop(take_surfaces());
    let options = OPTIONS.get().copied().unwrap_or_default();

    let host = match desktop::find() {
        Ok(host) => host,
        Err(error) => {
            eprintln!("wallflex: {error}; will retry");
            set_retry_delay();
            return;
        }
    };
    let instance = match module_instance() {
        Ok(instance) => instance,
        Err(error) => {
            eprintln!("wallflex: {error}");
            return;
        }
    };

    let mut surfaces = Vec::new();
    for monitor in monitors::list() {
        match surface::create(&host, monitor, instance, options) {
            Ok(surface) => surfaces.push(surface),
            Err(error) => eprintln!(
                "wallflex: no surface for the {}x{} monitor: {error}",
                monitor.width(),
                monitor.height()
            ),
        }
    }
    eprintln!(
        "wallflex: drawing on {} monitor(s), {:?} layout",
        surfaces.len(),
        host.layout
    );
    for surface in &surfaces {
        surface.invalidate();
    }
    STATE.with(|state| {
        if let Ok(mut state) = state.try_borrow_mut() {
            state.host = Some(host);
            state.surfaces = surfaces;
            state.skip_ticks = 0;
        }
    });
}

/// After a failed attempt, waits a few health checks before retrying.
fn set_retry_delay() {
    STATE.with(|state| {
        if let Ok(mut state) = state.try_borrow_mut() {
            state.skip_ticks = RETRY_SKIP_TICKS;
        }
    });
}
