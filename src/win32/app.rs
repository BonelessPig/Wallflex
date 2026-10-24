//! The main loop: a hidden controller window owns one surface per
//! monitor, checks on them every few seconds, and rebuilds them when
//! Explorer restarts, the displays change, the PC wakes up or the config
//! file is saved.

use std::cell::{Cell, RefCell};
use std::fmt;
use std::path::PathBuf;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};
use std::time::SystemTime;

use windows::Win32::Foundation::{
    HINSTANCE, HWND, LPARAM, LRESULT, POINT, WPARAM,
};
use windows::Win32::System::Console::SetConsoleCtrlHandler;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Shell::ShellExecuteW;
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, CreateWindowExW, DefWindowProcW, DestroyWindow,
    DispatchMessageW, FindWindowW, GA_ROOT, GW_HWNDNEXT, GetAncestor,
    GetMessageW, GetWindow, HHOOK, IsWindow, KillTimer, MSG, MSLLHOOKSTRUCT,
    PBT_APMRESUMEAUTOMATIC, PostMessageW, PostQuitMessage, RegisterClassExW,
    RegisterWindowMessageW, SW_SHOWNORMAL, SetTimer, SetWindowsHookExW,
    TranslateMessage, UnhookWindowsHookEx, WH_MOUSE_LL, WM_APP, WM_CLOSE,
    WM_DESTROY, WM_DISPLAYCHANGE, WM_LBUTTONDOWN, WM_LBUTTONUP,
    WM_POWERBROADCAST, WM_RBUTTONUP, WM_TIMER, WNDCLASSEXW, WS_EX_TOOLWINDOW,
    WS_POPUP, WindowFromPoint,
};
use windows::core::{BOOL, PCWSTR, w};

use super::desktop::{self, DesktopHost, Layout};
use super::discord::{BotSettings, DiscordLink};
use super::images::ImageFetcher;
use super::sensors::Sensors;
use super::startup;
use super::surface::{self, Surface};
use super::tray::{self, Choice, Tray};
use super::{class_name, monitors, struct_size};
use crate::args::Options;
use crate::config::{self, DiscordSettings, MonitorInfo, Settings};
use crate::discord::session::Credentials;
use crate::log;
use crate::widgets::Dashboard;

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
/// Timer that takes a sensor reading, refreshes the widgets and checks
/// the config file for changes.
const SENSOR_TIMER: usize = 3;
/// How often sensors are read, in milliseconds.
const SENSOR_INTERVAL_MS: u32 = 1_000;
/// Health checks to skip after a failed attempt, so a desktop that won't
/// split is asked again every 30 seconds rather than every 5.
const RETRY_SKIP_TICKS: u32 = 5;

/// Posted by the Discord thread when it has a new view.
const WM_DISCORD: u32 = WM_APP + 1;
/// Posted by the mouse hook when the desktop was clicked.
const WM_DESKTOP_CLICK: u32 = WM_APP + 2;
/// Posted by the image thread when a picture is ready.
const WM_IMAGES: u32 = WM_APP + 3;
/// Sent by the tray icon when it's clicked.
const WM_TRAY: u32 = WM_APP + 4;
/// The log's file name, next to the config.
const LOG_FILE: &str = "wallflex.log";
/// `ShellExecuteW` results above this mean success.
const SHELL_EXECUTE_OK: usize = 32;
/// A press and release this close together (pixels) count as a click.
const CLICK_SLOP: i32 = 4;
/// Top-level desktop windows: a click on one of these reached the
/// desktop rather than an app.
const DESKTOP_CLASSES: [&str; 2] = ["Progman", "WorkerW"];
/// File the Discord sign-in token is kept in, next to the config.
const DISCORD_TOKEN_FILE: &str = "discord-token.json";

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
    /// Sensor sources; `None` until `run` starts.
    sensors: Option<Sensors>,
    /// What the widgets show.
    dashboard: Dashboard,
    /// The current settings.
    settings: Settings,
    /// The config file, if there is one to watch.
    config: Option<ConfigFile>,
    /// The running Discord session, if Discord is set up.
    discord: Option<DiscordLink>,
    /// The settings it was started with.
    discord_settings: Option<DiscordSettings>,
    /// Where the Discord token is kept.
    token_path: Option<PathBuf>,
    /// The mouse hook, while a clickable widget may be showing.
    mouse_hook: Option<HHOOK>,
    /// Downloads pictures, started with the Discord session.
    images: Option<ImageFetcher>,
    /// The tray icon.
    tray: Option<Tray>,
}

/// The config file and when it was last loaded.
struct ConfigFile {
    /// Where it is.
    path: PathBuf,
    /// Its modification time when last loaded.
    modified: Option<SystemTime>,
}

impl ConfigFile {
    /// The file's current modification time, if it exists.
    fn modified_now(&self) -> Option<SystemTime> {
        std::fs::metadata(&self.path)
            .and_then(|m| m.modified())
            .ok()
    }

    /// Loads the file if it changed since the last load. Returns the new
    /// settings, or `None` if it didn't change or has a problem (which
    /// is logged; the old settings stay).
    fn reload_if_changed(&mut self) -> Option<Settings> {
        let now = self.modified_now();
        if now == self.modified {
            return None;
        }
        self.modified = now;
        self.load()
    }

    /// Reads and parses the file, logging any problem.
    fn load(&self) -> Option<Settings> {
        let path = self.path.display();
        let text = match std::fs::read_to_string(&self.path) {
            Ok(text) => text,
            Err(error) => {
                log!("can't read {path}: {error}; keeping the settings");
                return None;
            }
        };
        match config::parse(&text) {
            Ok(settings) => {
                log!("loaded settings from {path}");
                Some(settings)
            }
            Err(error) => {
                log!(
                    "{path} has a problem, so the previous settings stay:
{error}"
                );
                None
            }
        }
    }
}

thread_local! {
    /// The controller's state.
    static STATE: RefCell<State> = RefCell::new(State::default());
    /// Where the left button went down, for telling clicks from drags.
    static PRESSED_AT: Cell<Option<POINT>> = const { Cell::new(None) };
    /// The click waiting for `WM_DESKTOP_CLICK`.
    static CLICKED_AT: Cell<Option<POINT>> = const { Cell::new(None) };
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
/// `config_path` is the settings file to load and watch; without one the
/// built-in defaults are used.
pub fn run(
    options: Options,
    config_path: Option<PathBuf>,
) -> Result<(), AppError> {
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

    let token_path = config_path
        .as_ref()
        .and_then(|p| p.parent())
        .map(|dir| dir.join(DISCORD_TOKEN_FILE));
    let mut config = config_path.map(|path| ConfigFile {
        path,
        modified: None,
    });
    let settings = config
        .as_mut()
        .and_then(ConfigFile::reload_if_changed)
        .unwrap_or_default();
    STATE.with(|state| {
        if let Ok(mut state) = state.try_borrow_mut() {
            state.sensors = Some(Sensors::new());
            state.settings = settings;
            state.config = config;
            state.token_path = token_path;
            state.tray = Tray::add(controller, WM_TRAY);
        }
    });
    let _ = on_sensor_tick();

    rebuild();
    // SAFETY: our own window; no callback, so WM_TIMER is posted.
    if unsafe {
        SetTimer(Some(controller), SENSOR_TIMER, SENSOR_INTERVAL_MS, None)
    } == 0
    {
        return Err(windows::core::Error::from_thread().into());
    }
    // SAFETY: as above.
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

/// Paints the surface whose window is `hwnd`. Returns whether it was
/// found (it isn't while being created or during a rebuild).
pub(super) fn paint_surface(hwnd: HWND) -> bool {
    STATE.with(|state| {
        let Ok(mut state) = state.try_borrow_mut() else {
            return false;
        };
        let State {
            surfaces,
            dashboard,
            ..
        } = &mut *state;
        let found = surfaces.iter_mut().find(|s| s.hwnd() == hwnd);
        found.map(|surface| surface.paint(dashboard)).is_some()
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
        WM_DISCORD => {
            on_discord_update();
            return LRESULT(0);
        }
        WM_TRAY => {
            on_tray(hwnd, lparam);
            return LRESULT(0);
        }
        WM_IMAGES => {
            on_images_ready();
            return LRESULT(0);
        }
        WM_DESKTOP_CLICK => {
            if let Some(point) = CLICKED_AT.take() {
                on_desktop_click(point);
            }
            return LRESULT(0);
        }
        WM_DESTROY => {
            drop(take_surfaces());
            stop_discord();
            remove_mouse_hook();
            STATE.with(|state| {
                if let Ok(mut state) = state.try_borrow_mut() {
                    state.tray = None;
                }
            });
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
        SENSOR_TIMER if on_sensor_tick() => rebuild(),
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

/// Explorer restarted. Puts the tray icon back, and rebuilds unless a
/// rebuild is already scheduled or a health check has already put the
/// surfaces back.
fn on_explorer_restart(hwnd: HWND) {
    STATE.with(|state| {
        if let Ok(state) = state.try_borrow()
            && let Some(tray) = &state.tray
        {
            tray.show();
        }
    });
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

/// Takes a sensor reading, repaints the widgets and checks the config
/// file. Returns whether new settings were loaded (the surfaces need
/// rebuilding).
fn on_sensor_tick() -> bool {
    STATE.with(|state| {
        let Ok(mut state) = state.try_borrow_mut() else {
            return false;
        };
        let State {
            sensors,
            dashboard,
            surfaces,
            settings,
            config,
            ..
        } = &mut *state;
        if let Some(sensors) = sensors.as_mut() {
            dashboard.update(sensors.sample());
            for surface in surfaces.iter() {
                surface.invalidate_widgets(dashboard);
            }
        }
        match config.as_mut().and_then(ConfigFile::reload_if_changed) {
            Some(new) => {
                *settings = new;
                true
            }
            None => false,
        }
    })
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
                log!("can't restore window order: {error}");
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

/// Destroys the surfaces and creates fresh ones on every monitor, each
/// with the layout the settings give it.
fn rebuild() {
    drop(take_surfaces());
    apply_discord_settings();
    let Some(settings) = STATE.with(|state| {
        state.try_borrow().ok().map(|state| state.settings.clone())
    }) else {
        return;
    };
    // A --color on the command line beats the config file.
    let background = OPTIONS
        .get()
        .and_then(|o| o.background)
        .unwrap_or(settings.background);

    let host = match desktop::find() {
        Ok(host) => host,
        Err(error) => {
            log!("{error}; will retry");
            set_retry_delay();
            return;
        }
    };
    let instance = match module_instance() {
        Ok(instance) => instance,
        Err(error) => {
            log!("{error}");
            return;
        }
    };

    let mut surfaces = Vec::new();
    for monitor in monitors::list() {
        let info = MonitorInfo {
            name: &monitor.name,
            width: monitor.rect.width(),
            height: monitor.rect.height(),
            primary: monitor.primary,
        };
        let layout = settings.layout_for(&info);
        let short_name = monitor.name.trim_start_matches(r"\\.\");
        let widgets = layout.map_or(0, |l| l.widgets.len());
        log!(
            "{short_name} ({}x{}): {widgets} widget(s)",
            info.width,
            info.height
        );
        match surface::create(&host, &monitor, instance, background, layout) {
            Ok(surface) => surfaces.push(surface),
            Err(error) => log!("no surface for {short_name}: {error}"),
        }
    }
    log!(
        "drawing on {} monitor(s), {:?} layout",
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

/// Starts, restarts or stops the Discord session to match the settings.
fn apply_discord_settings() {
    let controller = CONTROLLER.load(Ordering::Acquire);
    STATE.with(|state| {
        let Ok(mut state) = state.try_borrow_mut() else {
            return;
        };
        let wanted = state.settings.discord.clone();
        state.dashboard.set_discord_configured(
            wanted.is_some(),
            wanted.as_ref().is_some_and(|d| d.bot_token.is_some()),
        );
        state.mouse_hook =
            install_mouse_hook(state.mouse_hook, wanted.is_some());
        if wanted == state.discord_settings {
            return;
        }
        // Stop the old session before starting a new one.
        state.discord = None;
        state.discord_settings.clone_from(&wanted);
        let (Some(settings), Some(token_path)) =
            (wanted, state.token_path.clone())
        else {
            return;
        };
        let bot = settings.bot_token.map(|token| BotSettings {
            token,
            friends: settings.friends,
        });
        let credentials = Credentials {
            client_id: settings.client_id,
            client_secret: settings.client_secret,
            redirect_uri: settings.redirect_uri,
        };
        let notify = HWND(std::ptr::with_exposed_provenance_mut(controller));
        state.discord = Some(DiscordLink::start(
            credentials,
            bot,
            token_path,
            notify,
            WM_DISCORD,
        ));
        if state.images.is_none() {
            state.images = Some(ImageFetcher::start(notify, WM_IMAGES));
        }
        log!("Discord: started");
    });
}

/// Installs or removes the mouse hook. The hook only watches for clicks
/// on the desktop; it never blocks or changes them. Returns the hook now
/// in place.
fn install_mouse_hook(current: Option<HHOOK>, wanted: bool) -> Option<HHOOK> {
    match (current, wanted) {
        (None, true) => {
            let instance = module_instance().ok()?;
            // SAFETY: a low-level hook with a plain function, called on
            // this thread's message loop.
            let hook = unsafe {
                SetWindowsHookExW(
                    WH_MOUSE_LL,
                    Some(mouse_hook),
                    Some(instance),
                    0,
                )
            };
            match hook {
                Ok(hook) => Some(hook),
                Err(error) => {
                    log!("can't watch for clicks: {error}");
                    None
                }
            }
        }
        (Some(hook), false) => {
            // SAFETY: the hook was installed by this program. If it's
            // already gone, there's nothing to undo.
            let _ = unsafe { UnhookWindowsHookEx(hook) };
            None
        }
        (current, _) => current,
    }
}

/// Removes the mouse hook (on shutdown).
fn remove_mouse_hook() {
    STATE.with(|state| {
        if let Ok(mut state) = state.try_borrow_mut() {
            state.mouse_hook = install_mouse_hook(state.mouse_hook, false);
        }
    });
}

/// The low-level mouse hook: notes left-button presses and, when the
/// release is at the same spot, posts the click to the controller.
/// Kept tiny: it runs for every mouse event on the system.
unsafe extern "system" fn mouse_hook(
    code: i32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    let message = u32::try_from(wparam.0).unwrap_or(0);
    if code >= 0 && (message == WM_LBUTTONDOWN || message == WM_LBUTTONUP) {
        let info = std::ptr::with_exposed_provenance::<MSLLHOOKSTRUCT>(
            lparam.0.cast_unsigned(),
        );
        // SAFETY: for WH_MOUSE_LL, lParam points to an MSLLHOOKSTRUCT
        // that's valid for the duration of the call.
        if let Some(info) = unsafe { info.as_ref() } {
            note_button(message, info.pt);
        }
    }
    // SAFETY: passes the event on unchanged, as hooks must.
    unsafe { CallNextHookEx(None, code, wparam, lparam) }
}

/// Remembers a press; on a release at the same spot, posts a click.
fn note_button(message: u32, point: POINT) {
    if message == WM_LBUTTONDOWN {
        PRESSED_AT.set(Some(point));
    } else if let Some(down) = PRESSED_AT.take() {
        let near = down.x.saturating_sub(point.x).abs() <= CLICK_SLOP
            && down.y.saturating_sub(point.y).abs() <= CLICK_SLOP;
        if near {
            CLICKED_AT.set(Some(point));
            post_to_controller(WM_DESKTOP_CLICK);
        }
    }
}

/// Posts `message` to the controller window.
fn post_to_controller(message: u32) {
    let address = CONTROLLER.load(Ordering::Acquire);
    if address != 0 {
        let hwnd = HWND(std::ptr::with_exposed_provenance_mut(address));
        // SAFETY: posting is safe from any thread; plain values. If it
        // fails, the click is just lost.
        let _ =
            unsafe { PostMessageW(Some(hwnd), message, WPARAM(0), LPARAM(0)) };
    }
}

/// Whether the desktop (not an app window) is under `point`.
fn desktop_under(point: POINT) -> bool {
    // SAFETY: both only read window state.
    let root = unsafe { GetAncestor(WindowFromPoint(point), GA_ROOT) };
    !root.is_invalid() && DESKTOP_CLASSES.contains(&class_name(root).as_str())
}

/// Handles a click on the desktop: if it hit a widget's control, acts on
/// it and repaints.
fn on_desktop_click(point: POINT) {
    if !desktop_under(point) {
        return;
    }
    STATE.with(|state| {
        let Ok(mut state) = state.try_borrow_mut() else {
            return;
        };
        let State {
            surfaces,
            dashboard,
            discord,
            images,
            ..
        } = &mut *state;
        let Some(action) = surfaces.iter().find_map(|s| s.hit_test(point))
        else {
            return;
        };
        let command = dashboard.click(&action);
        if let Some(link) = discord.as_ref() {
            for command in
                command.into_iter().chain(dashboard.discord_request())
            {
                link.send(command);
            }
        }
        fetch_wanted_images(dashboard, images.as_mut());
        for surface in surfaces.iter() {
            surface.invalidate_widgets(dashboard);
        }
    });
}

/// Asks the image thread for pictures the widgets now need.
fn fetch_wanted_images(
    dashboard: &mut Dashboard,
    images: Option<&mut ImageFetcher>,
) {
    if let Some(images) = images {
        for url in dashboard.wanted_images() {
            images.fetch(&url);
        }
    }
}

/// Pictures arrived: hand them to the widgets and repaint.
fn on_images_ready() {
    STATE.with(|state| {
        let Ok(mut state) = state.try_borrow_mut() else {
            return;
        };
        let State {
            images,
            dashboard,
            surfaces,
            ..
        } = &mut *state;
        let Some(images) = images.as_ref() else {
            return;
        };
        for (url, image) in images.take_ready() {
            dashboard.add_image(url, image);
        }
        for surface in surfaces.iter() {
            surface.invalidate_widgets(dashboard);
        }
    });
}

/// Stops the Discord session.
fn stop_discord() {
    STATE.with(|state| {
        if let Ok(mut state) = state.try_borrow_mut() {
            state.discord = None;
            state.discord_settings = None;
        }
    });
}

/// The Discord thread published a new view: show it.
fn on_discord_update() {
    STATE.with(|state| {
        let Ok(mut state) = state.try_borrow_mut() else {
            return;
        };
        let State {
            discord,
            dashboard,
            surfaces,
            images,
            ..
        } = &mut *state;
        let Some(link) = discord.as_ref() else {
            return;
        };
        dashboard.update_discord(link.view());
        dashboard.update_bot(link.bot_view());
        if let Some(command) = dashboard.discord_request() {
            link.send(command);
        }
        fetch_wanted_images(dashboard, images.as_mut());
        for surface in surfaces.iter() {
            surface.invalidate_widgets(dashboard);
        }
    });
}

/// The tray icon was clicked: shows the menu and does what was picked.
fn on_tray(hwnd: HWND, lparam: LPARAM) {
    let event = u32::try_from(lparam.0).unwrap_or(0);
    if event != WM_RBUTTONUP && event != WM_LBUTTONUP {
        return;
    }
    let Some(choice) = tray::show_menu(hwnd, startup::is_enabled()) else {
        return;
    };
    let config = STATE.with(|state| {
        state
            .try_borrow()
            .ok()
            .and_then(|s| s.config.as_ref().map(|c| c.path.clone()))
    });
    match choice {
        Choice::Settings => {
            if let Some(path) = &config {
                open_file(path);
            }
        }
        Choice::Log => {
            if let Some(dir) = config.as_ref().and_then(|p| p.parent()) {
                open_file(&dir.join(LOG_FILE));
            }
        }
        Choice::Startup => {
            let enable = !startup::is_enabled();
            match startup::set_enabled(enable) {
                Ok(()) => log!(
                    "start with Windows: {}",
                    if enable { "on" } else { "off" }
                ),
                Err(error) => {
                    log!("can't change starting with Windows: {error}");
                }
            }
        }
        Choice::Quit => post_to_controller(WM_CLOSE),
    }
}

/// Opens a file in its default app, or Notepad if it has none (a
/// `.toml` file often doesn't).
fn open_file(path: &std::path::Path) {
    let file = windows::core::HSTRING::from(path.as_os_str());
    // SAFETY: the strings are null-terminated and outlive the calls. The
    // results only signal success (above 32).
    let opened = unsafe {
        ShellExecuteW(
            None,
            w!("open"),
            &file,
            PCWSTR::null(),
            PCWSTR::null(),
            SW_SHOWNORMAL,
        )
    };
    if opened.0.addr() <= SHELL_EXECUTE_OK {
        // SAFETY: as above.
        let notepad = unsafe {
            ShellExecuteW(
                None,
                w!("open"),
                w!("notepad.exe"),
                &file,
                PCWSTR::null(),
                SW_SHOWNORMAL,
            )
        };
        if notepad.0.addr() <= SHELL_EXECUTE_OK {
            log!("can't open {}", path.display());
        }
    }
}
