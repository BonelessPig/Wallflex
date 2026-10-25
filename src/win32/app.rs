//! The main loop: a hidden controller window owns one surface per
//! monitor, checks on them every few seconds, and rebuilds them when
//! Explorer restarts, the displays change, the PC wakes up or the config
//! file is saved.

use std::cell::RefCell;
use std::fmt;
use std::path::PathBuf;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};
use std::time::{Instant, SystemTime};

use windows::Win32::Foundation::{
    HINSTANCE, HWND, LPARAM, LRESULT, POINT, WPARAM,
};
use windows::Win32::System::Console::SetConsoleCtrlHandler;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Shell::ShellExecuteW;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW,
    FindWindowW, GA_ROOT, GW_HWNDNEXT, GetAncestor, GetMessageW, GetWindow,
    IsWindow, KillTimer, MSG, PBT_APMRESUMEAUTOMATIC, PostMessageW,
    PostQuitMessage, RegisterClassExW, RegisterWindowMessageW, SW_SHOWNORMAL,
    SetTimer, TranslateMessage, WM_APP, WM_CLOSE, WM_DESTROY, WM_DISPLAYCHANGE,
    WM_LBUTTONUP, WM_POWERBROADCAST, WM_RBUTTONUP, WM_TIMER, WNDCLASSEXW,
    WS_EX_TOOLWINDOW, WS_POPUP, WindowFromPoint,
};
use windows::core::{BOOL, PCWSTR, w};

use super::audio::AudioTap;
use super::desktop::{self, DesktopHost, Layout};
use super::discord::{BotSettings, DiscordLink};
use super::images::ImageFetcher;
use super::layer;
use super::media::MediaWatcher;
use super::mouse::{self, MouseHook};
use super::sensors::Sensors;
use super::startup;
use super::surface::{self, Surface};
use super::tray::{self, Choice, Tray};
use super::usage::{self, CpuMeter};
use super::{class_name, monitors, struct_size};
use crate::args::Options;
use crate::backdrop::{Backdrop, Background};
use crate::config::{self, DiscordSettings, MonitorInfo, Settings};
use crate::discord::session::Credentials;
use crate::geometry::Rect;
use crate::log;
use crate::pace::{Governor, Level};
use crate::widgets::{Dashboard, Widget};

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
/// Milliseconds per full reading (GPUs, RAM, SSDs, config check, pace
/// check): once a second. Sensor ticks between them read only the CPU
/// and network; how often they come depends on the performance level.
const FULL_READING_MS: u32 = 1_000;
/// Wallflex's memory budget (private bytes). Over it, pictures are
/// dropped and the working set trimmed.
const MEMORY_BUDGET: u64 = 120 * 1024 * 1024;
/// Timer for the frames of a glide to a new reading; runs only while
/// the bars and rings are moving.
const FRAME_TIMER: usize = 4;

/// Timer for visualizer frames; runs only while sound plays or the bars
/// are still falling.
const VISUALIZER_TIMER: usize = 5;
/// Timer id for checking, while asleep, whether a widget is visible
/// again.
const WAKE_TIMER: usize = 6;
/// How often to check that, in milliseconds.
const WAKE_CHECK_MS: u32 = 1_000;

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
/// Posted by the mouse hook when the pointer moves over (or off) a
/// widget with hover highlights.
const WM_DESKTOP_HOVER: u32 = WM_APP + 5;
/// Posted by the audio thread when sound starts after silence.
const WM_AUDIO: u32 = WM_APP + 6;
/// Posted by the media thread when what's playing changes.
const WM_MEDIA: u32 = WM_APP + 7;
/// The log's file name, next to the config.
const LOG_FILE: &str = "wallflex.log";
/// The optional settings app, installed next to this exe.
const SETTINGS_APP: &str = "wallflex-settings.exe";
/// `ShellExecuteW` results above this mean success.
const SHELL_EXECUTE_OK: usize = 32;
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

/// Whether Wallflex is asleep.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Sleep {
    /// Running as usual.
    #[default]
    Awake,
    /// Every monitor with widgets is resting behind a full-screen
    /// window, so sensors, sound and frames are stopped.
    Asleep,
}

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
    mouse_hook: Option<MouseHook>,
    /// Downloads pictures, started with the Discord session.
    images: Option<ImageFetcher>,
    /// The tray icon.
    tray: Option<Tray>,
    /// When the last glide frame (or the reading that started it) was.
    last_frame: Option<Instant>,
    /// Sensor ticks so far, to know which are full readings.
    sensor_ticks: u32,
    /// The audio capture, while a layout has a visualizer.
    audio: Option<AudioTap>,
    /// What's playing, watched while a layout has a visualizer.
    media: Option<MediaWatcher>,
    /// When the last visualizer frame was (for a scrolling title).
    visualizer_frame: Option<Instant>,
    /// Reused buffer for the latest samples.
    samples: Vec<f32>,
    /// The visualizer timer is running.
    visualizer_running: bool,
    /// A full-screen app (a game, a video) is in front, so the
    /// visualizer rests.
    fullscreen: bool,
    /// Whether everything is stopped while the widgets are hidden.
    sleep: Sleep,
    /// Picks how smoothly to animate.
    governor: Option<Governor>,
    /// Wallflex's own CPU use.
    cpu_meter: CpuMeter,
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
                // Pictures are found from the config file's folder.
                let folder = self.path.parent().unwrap_or(&self.path);
                Some(settings.with_base_dir(folder))
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
    layer::register_class(instance)?;

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
            state.governor = Some(Governor::new(settings.performance));
            state.settings = settings;
            state.config = config;
            state.token_path = token_path;
            state.tray = Tray::add(controller, WM_TRAY);
        }
    });
    let _ = on_sensor_tick();

    rebuild();
    let level = current_level();
    apply_level(controller, level);
    // SAFETY: our own window; no callback, so WM_TIMER is posted.
    if unsafe {
        SetTimer(Some(controller), SENSOR_TIMER, level.sensor_ms(), None)
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
/// painted: not while it's being created, during a rebuild, or while it
/// rests (the default handler then just validates it).
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
        let painted = found.is_some_and(|surface| surface.paint(dashboard));
        if painted {
            let zones =
                surfaces.iter().flat_map(Surface::hover_zones).collect();
            mouse::set_zones(zones);
        }
        painted
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
            if let Some(point) = mouse::take_click() {
                on_desktop_click(point);
            }
            return LRESULT(0);
        }
        WM_AUDIO => {
            start_visualizer(hwnd);
            return LRESULT(0);
        }
        WM_MEDIA => {
            on_media_change();
            return LRESULT(0);
        }
        WM_DESKTOP_HOVER => {
            if let Some(point) = mouse::take_hover() {
                on_desktop_hover(point);
            }
            return LRESULT(0);
        }
        WM_DESTROY => {
            drop(take_surfaces());
            stop_discord();
            STATE.with(|state| {
                if let Ok(mut state) = state.try_borrow_mut() {
                    state.tray = None;
                    state.mouse_hook = None;
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
        HEALTH_TIMER => {
            on_health_tick(hwnd);
            check_memory();
        }
        SENSOR_TIMER => {
            let (reload, full) = on_sensor_tick();
            start_frames(hwnd);
            if full {
                check_fullscreen(hwnd);
                check_pace(hwnd);
                check_cover(hwnd);
            }
            if reload {
                rebuild();
            }
        }
        FRAME_TIMER => on_frame(hwnd),
        VISUALIZER_TIMER => on_visualizer_frame(hwnd),
        WAKE_TIMER => check_cover(hwnd),
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

/// Starts or stops the audio capture: it runs only while a layout has a
/// visualizer.
fn apply_audio_settings() {
    let controller = CONTROLLER.load(Ordering::Acquire);
    STATE.with(|state| {
        let Ok(mut state) = state.try_borrow_mut() else {
            return;
        };
        let wanted = state.sleep == Sleep::Awake
            && state.settings.layouts.iter().any(|layout| {
                layout
                    .widgets
                    .iter()
                    .any(|p| p.widget == Widget::Visualizer)
            });
        if wanted && state.audio.is_none() {
            let notify =
                HWND(std::ptr::with_exposed_provenance_mut(controller));
            state.audio = Some(AudioTap::start(notify, WM_AUDIO));
            state.media = Some(MediaWatcher::start(notify, WM_MEDIA));
        } else if !wanted {
            state.audio = None;
            state.media = None;
        }
    });
}

/// What's playing changed: shows it under the visualizer.
fn on_media_change() {
    STATE.with(|state| {
        let Ok(mut state) = state.try_borrow_mut() else {
            return;
        };
        let Some(change) = state.media.as_ref().and_then(MediaWatcher::take)
        else {
            return;
        };
        state.dashboard.set_now_playing(change);
        for surface in &state.surfaces {
            surface.invalidate_widgets(&state.dashboard);
        }
    });
}

/// Sound started: runs the visualizer frames, unless a full-screen app
/// is in front.
fn start_visualizer(hwnd: HWND) {
    let start = STATE.with(|state| {
        let Ok(mut state) = state.try_borrow_mut() else {
            return false;
        };
        let start = state.audio.is_some()
            && !state.fullscreen
            && !state.visualizer_running;
        if start {
            state.visualizer_running = true;
        }
        start
    });
    if start {
        let interval = current_level().visualizer_ms();
        // SAFETY: our own window; no callback, so WM_TIMER is posted. A
        // failure only means the bars stay still.
        let _ =
            unsafe { SetTimer(Some(hwnd), VISUALIZER_TIMER, interval, None) };
    }
}

/// One visualizer frame: the latest sound to bars, repainting only the
/// visualizer. Stops once the sound has stopped and the bars are down.
fn on_visualizer_frame(hwnd: HWND) {
    let keep = STATE.with(|state| {
        let Ok(mut state) = state.try_borrow_mut() else {
            return true;
        };
        let State {
            audio,
            samples,
            dashboard,
            surfaces,
            visualizer_running,
            visualizer_frame,
            ..
        } = &mut *state;
        let Some(tap) = audio.as_ref() else {
            *visualizer_running = false;
            return false;
        };
        let loud = tap.is_loud();
        let rate = tap.latest(samples);
        if !loud {
            // The last sound stays in the buffer; let the bars fall.
            samples.clear();
        }
        let now = Instant::now();
        let elapsed = visualizer_frame
            .map_or_else(Default::default, |last| now.duration_since(last));
        *visualizer_frame = Some(now);
        dashboard.feed_audio(samples, rate, elapsed);
        for surface in surfaces.iter() {
            surface.invalidate_visualizer(dashboard);
        }
        let keep = loud || !dashboard.visualizer_resting();
        *visualizer_running = keep;
        keep
    });
    if !keep {
        // SAFETY: our own timer; failure only means it was already gone.
        let _ = unsafe { KillTimer(Some(hwnd), VISUALIZER_TIMER) };
    }
}

/// Once a second: notes whether a full-screen app is in front. The
/// visualizer rests while one is, and picks up again afterwards.
fn check_fullscreen(hwnd: HWND) {
    let front = foreground_rect();
    let (stop, resume) = STATE.with(|state| {
        let Ok(mut state) = state.try_borrow_mut() else {
            return (false, false);
        };
        // Only a window covering a monitor that shows the visualizer
        // hides it; a full-screen video on another monitor doesn't.
        let fullscreen =
            front.is_some_and(|window| {
                state
                    .surfaces
                    .iter()
                    .any(|s| s.visualizer_hidden_by(window))
            }) || all_resting(&state.surfaces, Surface::shows_visualizer);
        let was = state.fullscreen;
        state.fullscreen = fullscreen;
        let stop = fullscreen && state.visualizer_running;
        if stop {
            state.visualizer_running = false;
            state.dashboard.rest_visualizer();
            for surface in &state.surfaces {
                surface.invalidate_visualizer(&state.dashboard);
            }
        }
        let resume = was
            && !fullscreen
            && state.audio.as_ref().is_some_and(AudioTap::is_loud);
        (stop, resume)
    });
    if stop {
        // SAFETY: our own timer; failure only means it was already gone.
        let _ = unsafe { KillTimer(Some(hwnd), VISUALIZER_TIMER) };
    }
    if resume {
        start_visualizer(hwnd);
    }
}

/// The performance level now.
fn current_level() -> Level {
    STATE.with(|state| {
        state
            .try_borrow()
            .ok()
            .map_or(Level::Normal, |s| current_level_of(s.governor.as_ref()))
    })
}

/// `governor`'s level, or Normal before it exists.
fn current_level_of(governor: Option<&Governor>) -> Level {
    governor.map_or(Level::Normal, Governor::level)
}

/// The config's performance mode changed: switches the governor.
fn apply_performance_settings() {
    let controller = CONTROLLER.load(Ordering::Acquire);
    let changed = STATE.with(|state| {
        let mut state = state.try_borrow_mut().ok()?;
        let mode = state.settings.performance;
        state.governor.as_mut()?.set_mode(mode)
    });
    if let Some(level) = changed {
        let hwnd = HWND(std::ptr::with_exposed_provenance_mut(controller));
        log!("performance: {}", level.name());
        apply_level(hwnd, level);
    }
}

/// Once a second: lets the governor step the level up or down for the
/// PC's load and Wallflex's own CPU use.
fn check_pace(hwnd: HWND) {
    let changed = STATE.with(|state| {
        let mut state = state.try_borrow_mut().ok()?;
        let own = state.cpu_meter.sample();
        let system = state.dashboard.cpu_total();
        let level = state.governor.as_mut()?.tick(system, own)?;
        Some((level, system, own))
    });
    if let Some((level, system, own)) = changed {
        log!(
            "performance: now {} (PC {:.0}% busy, Wallflex {own:.2}%)",
            level.name(),
            system.unwrap_or(0.0)
        );
        apply_level(hwnd, level);
    }
}

/// Puts `level` into effect: glide length, sensor rate, and the rate of
/// any frames already running.
fn apply_level(hwnd: HWND, level: Level) {
    let (frames, visualizer) = STATE.with(|state| {
        let Ok(mut state) = state.try_borrow_mut() else {
            return (false, false);
        };
        state.dashboard.set_pace(level.pace());
        (state.dashboard.is_moving(), state.visualizer_running)
    });
    // SAFETY: our own window and timers; resetting a timer replaces its
    // interval. Failures only leave the old rate in place.
    unsafe {
        let _ = SetTimer(Some(hwnd), SENSOR_TIMER, level.sensor_ms(), None);
        if frames {
            let _ = SetTimer(Some(hwnd), FRAME_TIMER, level.frame_ms(), None);
        }
        if visualizer {
            let _ = SetTimer(
                Some(hwnd),
                VISUALIZER_TIMER,
                level.visualizer_ms(),
                None,
            );
        }
    }
}

/// Every few seconds: if Wallflex is over its memory budget, drops the
/// downloaded pictures and hands unused memory back to Windows.
fn check_memory() {
    let Some(private) = usage::private_bytes() else {
        return;
    };
    if private <= MEMORY_BUDGET {
        return;
    }
    log!(
        "memory: {} MB is over the budget; freeing pictures",
        private / (1024 * 1024)
    );
    STATE.with(|state| {
        if let Ok(mut state) = state.try_borrow_mut() {
            state.dashboard.clear_images();
        }
    });
    usage::trim_working_set();
}

/// The window in front, in desktop pixels, unless it's the desktop
/// itself (or there isn't one).
fn foreground_rect() -> Option<Rect> {
    use windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow;
    // SAFETY: a plain query about the window in front.
    app_window_rect(unsafe { GetForegroundWindow() })
}

/// The rectangle (desktop pixels) of the app window `window` belongs
/// to, unless it's the desktop itself (or there isn't one).
fn app_window_rect(window: HWND) -> Option<Rect> {
    use windows::Win32::Foundation::RECT;
    use windows::Win32::UI::WindowsAndMessaging::GetWindowRect;
    if window.is_invalid() {
        return None;
    }
    // SAFETY: a plain query about a window handle.
    let root = unsafe { GetAncestor(window, GA_ROOT) };
    if DESKTOP_CLASSES.contains(&class_name(root).as_str()) {
        return None;
    }
    let mut rect = RECT::default();
    // SAFETY: a valid window and a place to write.
    unsafe { GetWindowRect(root, &raw mut rect) }.ok()?;
    Some(super::to_rect(rect))
}

/// Whether an app window covers all of `monitor` (desktop pixels): the
/// window on top at its middle, if that isn't the desktop.
fn covered(monitor: Rect) -> bool {
    let middle = monitor.center();
    let point = POINT {
        x: middle.x,
        y: middle.y,
    };
    // SAFETY: a plain query about what's on screen at a point.
    let window = unsafe { WindowFromPoint(point) };
    app_window_rect(window).is_some_and(|rect| rect.covers(monitor))
}

/// Whether there are surfaces that `pick` chooses and all of them are
/// resting.
fn all_resting(surfaces: &[Surface], pick: fn(&Surface) -> bool) -> bool {
    let mut picked = surfaces.iter().filter(|s| pick(s)).peekable();
    picked.peek().is_some() && picked.all(Surface::is_resting)
}

/// Once a second (or, while asleep, on the wake timer): in Light, rests
/// each monitor a full-screen window covers; wakes it once it's
/// uncovered. While every monitor with widgets rests, Wallflex sleeps.
fn check_cover(hwnd: HWND) {
    let light = current_level() == Level::Light;
    let (sleep, wake) = STATE.with(|state| {
        let Ok(mut state) = state.try_borrow_mut() else {
            return (false, false);
        };
        for surface in state.surfaces.iter_mut().filter(|s| s.has_widgets()) {
            let covered = covered(surface.monitor());
            if covered && light && !surface.is_resting() {
                surface.rest();
            } else if !covered && surface.is_resting() {
                surface.wake();
            }
        }
        let hidden = all_resting(&state.surfaces, Surface::has_widgets);
        let asleep = state.sleep == Sleep::Asleep;
        (hidden && !asleep, !hidden && asleep)
    });
    if sleep {
        fall_asleep(hwnd);
    } else if wake {
        wake_up(hwnd);
    }
}

/// Every widget is hidden: stops the sensors, frames and sound, frees
/// the pictures and hands memory back to Windows, and checks once a
/// second for a widget to come back into view.
fn fall_asleep(hwnd: HWND) {
    // SAFETY: our own timers; failure only means one was already gone.
    unsafe {
        let _ = KillTimer(Some(hwnd), SENSOR_TIMER);
        let _ = KillTimer(Some(hwnd), FRAME_TIMER);
        let _ = KillTimer(Some(hwnd), VISUALIZER_TIMER);
    }
    let threads = STATE.with(|state| {
        let mut state = state.try_borrow_mut().ok()?;
        state.sleep = Sleep::Asleep;
        state.visualizer_running = false;
        state.dashboard.rest_visualizer();
        state.dashboard.clear_images();
        Some((state.audio.take(), state.media.take()))
    });
    // Stops the capture and media threads, outside the borrow.
    drop(threads);
    usage::trim_working_set();
    log!("asleep: every widget is behind a full-screen window");
    // SAFETY: our own window; no callback, so WM_TIMER is posted.
    if unsafe { SetTimer(Some(hwnd), WAKE_TIMER, WAKE_CHECK_MS, None) } == 0 {
        // Without the check nothing would wake it: stay awake instead.
        wake_up(hwnd);
    }
}

/// A widget is visible again: takes a fresh reading and restarts the
/// sensors, sound and pictures.
fn wake_up(hwnd: HWND) {
    // SAFETY: our own timer; failure only means it was already gone.
    let _ = unsafe { KillTimer(Some(hwnd), WAKE_TIMER) };
    STATE.with(|state| {
        if let Ok(mut state) = state.try_borrow_mut() {
            state.sleep = Sleep::Awake;
            let State {
                dashboard, images, ..
            } = &mut *state;
            fetch_wanted_images(dashboard, images.as_mut());
        }
    });
    log!("awake");
    apply_audio_settings();
    let (reload, _) = on_sensor_tick();
    if reload {
        rebuild();
    }
    let level = current_level();
    // SAFETY: our own window; no callback, so WM_TIMER is posted.
    if unsafe { SetTimer(Some(hwnd), SENSOR_TIMER, level.sensor_ms(), None) }
        == 0
    {
        log!("couldn't restart the sensor timer");
    }
}

/// Starts the glide frames if the last reading set the bars and rings
/// moving.
fn start_frames(hwnd: HWND) {
    let moving = STATE.with(|state| {
        let Ok(mut state) = state.try_borrow_mut() else {
            return false;
        };
        state.last_frame = Some(Instant::now());
        state.dashboard.is_moving()
    });
    if moving {
        let interval = current_level().frame_ms();
        // SAFETY: our own window; no callback, so WM_TIMER is posted. A
        // failure only means the bars jump instead of gliding.
        let _ = unsafe { SetTimer(Some(hwnd), FRAME_TIMER, interval, None) };
    }
}

/// One glide frame: moves the bars and rings on and repaints just them,
/// stopping the frames once they've arrived.
fn on_frame(hwnd: HWND) {
    let moving = STATE.with(|state| {
        let Ok(mut state) = state.try_borrow_mut() else {
            return true;
        };
        let State {
            dashboard,
            surfaces,
            last_frame,
            ..
        } = &mut *state;
        let now = Instant::now();
        let elapsed = last_frame
            .map_or_else(Default::default, |last| now.duration_since(last));
        *last_frame = Some(now);
        let moving = dashboard.advance(elapsed);
        for surface in surfaces.iter() {
            surface.invalidate_moving(dashboard);
        }
        moving
    });
    if !moving {
        // SAFETY: our own timer; failure only means it was already gone.
        let _ = unsafe { KillTimer(Some(hwnd), FRAME_TIMER) };
    }
}

/// Takes a sensor reading, repaints the widgets and, on a full reading,
/// checks the config file. Returns whether new settings were loaded (the
/// surfaces need rebuilding) and whether this was a full reading.
fn on_sensor_tick() -> (bool, bool) {
    STATE.with(|state| {
        let Ok(mut state) = state.try_borrow_mut() else {
            return (false, false);
        };
        let State {
            sensors,
            dashboard,
            surfaces,
            settings,
            config,
            sensor_ticks,
            governor,
            ..
        } = &mut *state;
        let per_full = FULL_READING_MS
            .checked_div(current_level_of(governor.as_ref()).sensor_ms())
            .unwrap_or(1)
            .max(1);
        let full = sensor_ticks.is_multiple_of(per_full);
        *sensor_ticks = sensor_ticks.wrapping_add(1);
        if let Some(sensors) = sensors.as_mut() {
            if full {
                dashboard.update(sensors.sample());
                for surface in surfaces.iter() {
                    surface.invalidate_widgets(dashboard);
                }
            } else {
                dashboard.update(sensors.sample_quick());
                for surface in surfaces.iter() {
                    surface.invalidate_quick(dashboard);
                }
            }
        }
        if !full {
            return (false, false);
        }
        match config.as_mut().and_then(ConfigFile::reload_if_changed) {
            Some(new) => {
                *settings = new;
                (true, true)
            }
            None => (false, true),
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
        let ours = below_icons.is_some_and(|hwnd| {
            surfaces.iter().any(|surface| surface.owns(hwnd))
        });
        if !ours {
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
    apply_audio_settings();
    apply_performance_settings();
    STATE.with(|state| {
        if let Ok(mut state) = state.try_borrow_mut() {
            let kbps = state.settings.net_max_mbps.map(|mbps| mbps * 1_000.0);
            state.dashboard.set_net_limit(kbps);
            let seconds = state.settings.net_seconds;
            state.dashboard.set_net_seconds(seconds);
            let parts = state.settings.parts.clone();
            state.dashboard.set_parts(parts);
        }
    });
    let Some(settings) = STATE.with(|state| {
        state.try_borrow().ok().map(|state| state.settings.clone())
    }) else {
        return;
    };
    // A --color on the command line beats the config file.
    let color = OPTIONS.get().and_then(|o| o.background);

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
        let backdrop = match color {
            Some(color) => Backdrop {
                background: Background::Color(color),
                image: None,
            },
            None => settings.backdrop_for(layout),
        };
        if let Some(image) = &backdrop.image {
            log!("{short_name}: picture {}", image.path.display());
        }
        match surface::create(&host, &monitor, instance, &backdrop, layout) {
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
        if wanted.is_some() && state.mouse_hook.is_none() {
            let notify =
                HWND(std::ptr::with_exposed_provenance_mut(controller));
            state.mouse_hook = Some(MouseHook::start(
                notify,
                WM_DESKTOP_CLICK,
                WM_DESKTOP_HOVER,
            ));
        } else if wanted.is_none() {
            state.mouse_hook = None;
        }
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

/// The pointer moved over (or off) a widget: highlights the control
/// under it, repainting only when that changes.
fn on_desktop_hover(point: POINT) {
    STATE.with(|state| {
        let Ok(mut state) = state.try_borrow_mut() else {
            return;
        };
        let State {
            surfaces,
            dashboard,
            ..
        } = &mut *state;
        let over = surfaces.iter().find_map(|s| s.hit_test(point));
        // Only the desktop itself counts, not a window above it.
        let hover = over.filter(|_| desktop_under(point));
        mouse::set_hovering(hover.is_some());
        if dashboard.set_hover(hover) {
            for surface in surfaces.iter() {
                surface.invalidate_hoverable();
            }
        }
    });
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
            sleep,
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
        if *sleep == Sleep::Awake {
            fetch_wanted_images(dashboard, images.as_mut());
        }
        // Only the Discord widget shows Discord; repainting the rest for
        // every presence or speaking change was wasted work.
        for surface in surfaces.iter() {
            surface.invalidate_widget(dashboard, Widget::Discord);
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
            if let Some(path) = &config
                && !open_settings_app(path)
            {
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

/// Starts the optional settings app (`wallflex-settings.exe`, next to
/// this exe) on `config`. Returns false if it isn't installed or won't
/// start, so the caller can open the file instead.
fn open_settings_app(config: &std::path::Path) -> bool {
    let Some(app) = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|dir| dir.join(SETTINGS_APP)))
        .filter(|app| app.is_file())
    else {
        return false;
    };
    match std::process::Command::new(&app)
        .arg("--config")
        .arg(config)
        .spawn()
    {
        Ok(_child) => true,
        Err(error) => {
            log!("can't start {}: {error}", app.display());
            false
        }
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
