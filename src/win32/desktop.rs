//! Finding the layer between the wallpaper and the desktop icons.
//!
//! None of this is documented by Microsoft. Explorer splits the desktop
//! when sent message `0x052C`, and the split takes one of two shapes:
//!
//! - **Raised** (Windows 11 24H2 and later): the icons
//!   (`SHELLDLL_DefView`) stay a child of `Progman`, next to a `WorkerW`
//!   child that draws the wallpaper. Wallflex adds its windows to
//!   `Progman`, directly below the icons.
//! - **Classic** (earlier versions): the icons move into a top-level
//!   `WorkerW`, and the next top-level `WorkerW` draws the wallpaper.
//!   Wallflex adds its windows inside that one.

use std::fmt::{self, Write as _};

use windows::Win32::Foundation::{HWND, LPARAM, RECT, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::{
    FindWindowExW, FindWindowW, GW_CHILD, GW_HWNDNEXT, GetWindow,
    GetWindowRect, GetWindowThreadProcessId, IsWindowVisible, SMTO_NORMAL,
    SendMessageTimeoutW,
};
use windows::core::{PCWSTR, w};

use super::{class_name, monitors, to_rect};

/// Asks Explorer to split the desktop (undocumented).
const SPLIT_MESSAGE: u32 = 0x052C;
/// The `wParam` for [`SPLIT_MESSAGE`]; `0xD` is the value that works on
/// current Windows 11 builds, where 0 is ignored.
const SPLIT_WPARAM: usize = 0xD;
/// The `lParam` for [`SPLIT_MESSAGE`].
const SPLIT_LPARAM: isize = 1;
/// How long to wait for Explorer to answer, in milliseconds.
const SPLIT_TIMEOUT_MS: u32 = 1_000;

/// Class of the window holding the desktop icons.
const ICONS_CLASS: PCWSTR = w!("SHELLDLL_DefView");
/// Class of Explorer's desktop worker windows.
const WORKER_CLASS: PCWSTR = w!("WorkerW");
/// Class of Explorer's desktop root window.
const PROGMAN_CLASS: PCWSTR = w!("Progman");

/// The shape of Explorer's split desktop.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Layout {
    /// Windows 11 24H2+: draw inside `Progman`, below the icons.
    Raised,
    /// Earlier versions: draw inside the wallpaper `WorkerW`.
    Classic,
}

/// Where Wallflex's windows go.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DesktopHost {
    /// The window to create surfaces inside.
    pub parent: HWND,
    /// The desktop icons window; surfaces must stay below it.
    pub icons: HWND,
    /// Which shape the desktop has.
    pub layout: Layout,
}

/// Why no host was found.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HostError {
    /// Explorer's desktop window doesn't exist (Explorer not running).
    NoProgman,
    /// Explorer didn't split the desktop when asked.
    NoSplit,
}

impl fmt::Display for HostError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoProgman => write!(f, "Explorer's desktop isn't running"),
            Self::NoSplit => {
                write!(f, "Explorer didn't make room behind the icons")
            }
        }
    }
}

/// Finds the host, asking Explorer to split the desktop only when it
/// isn't split already (each request can add another worker window).
///
/// # Errors
///
/// [`HostError`] when Explorer isn't running or won't split.
pub fn find() -> Result<DesktopHost, HostError> {
    let progman = progman().ok_or(HostError::NoProgman)?;
    if let Some(host) = detect(progman) {
        return Ok(host);
    }
    request_split(progman);
    detect(progman).ok_or(HostError::NoSplit)
}

/// Describes Explorer's desktop windows for `--diagnose`, without asking
/// Explorer to change anything.
pub fn describe() -> String {
    let mut out = String::new();
    let Some(progman) = progman() else {
        return "Progman not found: Explorer's desktop isn't running."
            .to_owned();
    };
    let _ = writeln!(out, "Progman {}", window_line(progman));
    let _ = writeln!(out, "  children, front to back:");
    for child in children(progman) {
        let _ = writeln!(out, "    {}", window_line(child));
    }

    let _ = writeln!(out, "Top-level WorkerW windows:");
    for worker in top_level_workers() {
        let holds_icons = find_child(Some(worker), None, ICONS_CLASS)
            .map_or("", |_| " (holds the icons)");
        let _ = writeln!(out, "  {}{holds_icons}", window_line(worker));
    }

    let detected = match detect(progman) {
        Some(host) => format!("{:?}", host.layout),
        None => "not split yet (Wallflex will ask Explorer)".to_owned(),
    };
    let _ = writeln!(out, "Layout: {detected}");

    let _ = writeln!(out, "Monitors:");
    for m in monitors::list() {
        let _ = writeln!(
            out,
            "  {}x{} at {}, {}",
            m.width(),
            m.height(),
            m.left,
            m.top
        );
    }
    out
}

/// Explorer's `Progman` window, if Explorer is running.
fn progman() -> Option<HWND> {
    // SAFETY: both strings are valid, null-terminated constants.
    unsafe { FindWindowW(PROGMAN_CLASS, PCWSTR::null()) }.ok()
}

/// Looks for a split desktop of either shape.
fn detect(progman: HWND) -> Option<DesktopHost> {
    if let Some(icons) = find_child(Some(progman), None, ICONS_CLASS)
        && find_child(Some(progman), None, WORKER_CLASS).is_some()
    {
        return Some(DesktopHost {
            parent: progman,
            icons,
            layout: Layout::Raised,
        });
    }

    let explorer = process_id(progman);
    for worker in top_level_workers() {
        if let Some(icons) = find_child(Some(worker), None, ICONS_CLASS) {
            let layer = find_child(None, Some(worker), WORKER_CLASS)?;
            // Other programs can create "WorkerW" windows too.
            return (process_id(layer) == explorer).then_some(DesktopHost {
                parent: layer,
                icons,
                layout: Layout::Classic,
            });
        }
    }
    None
}

/// Sends Explorer the split request and waits up to a second.
fn request_split(progman: HWND) {
    // SAFETY: plain values only; no result pointer is passed. A timeout
    // or failure shows up as a failed detect afterwards, so the result
    // is deliberately ignored.
    let _ = unsafe {
        SendMessageTimeoutW(
            progman,
            SPLIT_MESSAGE,
            WPARAM(SPLIT_WPARAM),
            LPARAM(SPLIT_LPARAM),
            SMTO_NORMAL,
            SPLIT_TIMEOUT_MS,
            None,
        )
    };
}

/// `FindWindowExW` by class: the first match under `parent` (or among
/// top-level windows when `None`) that comes after `after`.
fn find_child(
    parent: Option<HWND>,
    after: Option<HWND>,
    class: PCWSTR,
) -> Option<HWND> {
    // SAFETY: `class` is a valid null-terminated constant; the handles are
    // only compared by Windows, never dereferenced.
    unsafe { FindWindowExW(parent, after, class, PCWSTR::null()) }.ok()
}

/// All top-level `WorkerW` windows, front to back.
fn top_level_workers() -> Vec<HWND> {
    let mut workers = Vec::new();
    let mut after = None;
    while let Some(worker) = find_child(None, after, WORKER_CLASS) {
        workers.push(worker);
        after = Some(worker);
    }
    workers
}

/// The direct children of `parent`, front to back (z-order).
fn children(parent: HWND) -> Vec<HWND> {
    let mut found = Vec::new();
    // SAFETY: GetWindow only reads window relationships.
    let mut next = unsafe { GetWindow(parent, GW_CHILD) }.ok();
    while let Some(child) = next {
        found.push(child);
        // SAFETY: as above.
        next = unsafe { GetWindow(child, GW_HWNDNEXT) }.ok();
    }
    found
}

/// The id of the process that owns `hwnd`, or 0 if it's gone.
fn process_id(hwnd: HWND) -> u32 {
    let mut pid = 0_u32;
    // SAFETY: `pid` is a valid place for the call to write to.
    let _thread = unsafe { GetWindowThreadProcessId(hwnd, Some(&raw mut pid)) };
    pid
}

/// One line describing a window: class, handle, process, visibility and
/// rectangle.
fn window_line(hwnd: HWND) -> String {
    let mut rect = RECT::default();
    // SAFETY: `rect` is a valid place for the call to write to. On
    // failure it stays zero, which the line then shows.
    let _ = unsafe { GetWindowRect(hwnd, &raw mut rect) };
    // SAFETY: only reads the window's style.
    let visible = unsafe { IsWindowVisible(hwnd) }.as_bool();
    let r = to_rect(rect);
    format!(
        "{} {:?} pid {} {} {}x{} at {}, {}",
        class_name(hwnd),
        hwnd.0,
        process_id(hwnd),
        if visible { "visible" } else { "hidden" },
        r.width(),
        r.height(),
        r.left,
        r.top
    )
}
