//! Watches the mouse for clicks and hovers on the desktop widgets, with a
//! low-level hook on a thread of its own. Windows waits for a low-level
//! hook before it moves the cursor, so the hook must never sit behind
//! painting: this thread does nothing else, and only hands positions to
//! the controller window. It never blocks or changes the mouse's events.

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Mutex, mpsc};
use std::thread::{self, JoinHandle};

use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, GetMessageW, MSG, MSLLHOOKSTRUCT, PostMessageW,
    PostThreadMessageW, SetWindowsHookExW, UnhookWindowsHookEx, WH_MOUSE_LL,
    WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEMOVE, WM_QUIT,
};

use crate::geometry::Rect;
use crate::log;

/// A press and release this close together (pixels) count as a click.
const CLICK_SLOP: i32 = 4;
/// Stack size for the hook thread (it only runs the tiny hook).
const THREAD_STACK: usize = 64 * 1024;

/// What the hook thread and the controller share.
struct Shared {
    /// The controller window's address.
    notify: usize,
    /// Posted for a click.
    click_message: u32,
    /// Posted for a pointer move over a widget.
    hover_message: u32,
    /// Where the left button went down, for telling clicks from drags.
    pressed: Option<POINT>,
    /// The click waiting for the controller.
    clicked: Option<POINT>,
    /// The latest pointer position waiting for the controller.
    hover_at: Option<POINT>,
    /// Where widgets with hover highlights are (desktop pixels).
    zones: Vec<Rect>,
}

/// Shared with the hook, which is a plain function.
static SHARED: Mutex<Shared> = Mutex::new(Shared {
    notify: 0,
    click_message: 0,
    hover_message: 0,
    pressed: None,
    clicked: None,
    hover_at: None,
    zones: Vec::new(),
});
/// A hover message is already on its way, so moves are batched.
static HOVER_PENDING: AtomicBool = AtomicBool::new(false);
/// Something is highlighted, so leaving a zone must clear it.
static HOVERING: AtomicBool = AtomicBool::new(false);
/// The hook thread's id, for telling it to stop.
static THREAD_ID: AtomicU32 = AtomicU32::new(0);

/// The running hook; removed on drop.
pub struct MouseHook {
    /// The hook thread, joined on drop.
    thread: Option<JoinHandle<()>>,
}

impl MouseHook {
    /// Starts watching. Clicks are announced by posting `click_message`
    /// to `notify`, pointer moves over widgets by `hover_message`.
    pub fn start(notify: HWND, click_message: u32, hover_message: u32) -> Self {
        if let Ok(mut shared) = SHARED.lock() {
            shared.notify = notify.0.expose_provenance();
            shared.click_message = click_message;
            shared.hover_message = hover_message;
        }
        let (ready, started) = mpsc::channel();
        let thread = thread::Builder::new()
            .name("mouse".into())
            .stack_size(THREAD_STACK)
            .spawn(move || run(&ready));
        let thread = match thread {
            Ok(thread) => {
                // Wait until the hook is in (or failed), so stopping
                // right away still finds the thread.
                let _ = started.recv();
                Some(thread)
            }
            Err(error) => {
                log!("can't start the mouse thread: {error}");
                None
            }
        };
        Self { thread }
    }
}

impl Drop for MouseHook {
    fn drop(&mut self) {
        let id = THREAD_ID.swap(0, Ordering::AcqRel);
        if id != 0 {
            // SAFETY: plain values; the thread's message loop ends on
            // WM_QUIT. If it's already gone, there's nothing to stop.
            let _ = unsafe {
                PostThreadMessageW(id, WM_QUIT, WPARAM(0), LPARAM(0))
            };
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// The click waiting for the controller, if any.
pub fn take_click() -> Option<POINT> {
    SHARED.lock().ok()?.clicked.take()
}

/// The pointer position waiting for the controller, if any; lets the
/// next move post again.
pub fn take_hover() -> Option<POINT> {
    HOVER_PENDING.store(false, Ordering::Release);
    SHARED.lock().ok()?.hover_at.take()
}

/// Sets where widgets with hover highlights are (desktop pixels).
pub fn set_zones(zones: Vec<Rect>) {
    if let Ok(mut shared) = SHARED.lock() {
        shared.zones = zones;
    }
}

/// Notes whether something is highlighted (so leaving it is noticed).
pub fn set_hovering(hovering: bool) {
    HOVERING.store(hovering, Ordering::Release);
}

/// The hook thread: installs the hook and pumps messages (which is when
/// Windows calls it) until told to quit.
fn run(ready: &mpsc::Sender<()>) {
    // SAFETY: plain query.
    THREAD_ID.store(unsafe { GetCurrentThreadId() }, Ordering::Release);
    // SAFETY: this program's own module handle.
    let instance = unsafe { GetModuleHandleW(None) };
    // SAFETY: a low-level hook with a plain function, called on this
    // thread's message loop.
    let hook = instance.and_then(|module| unsafe {
        SetWindowsHookExW(WH_MOUSE_LL, Some(mouse_hook), Some(module.into()), 0)
    });
    let _ = ready.send(());
    let hook = match hook {
        Ok(hook) => hook,
        Err(error) => {
            log!("can't watch for clicks: {error}");
            THREAD_ID.store(0, Ordering::Release);
            return;
        }
    };
    let mut message = MSG::default();
    // SAFETY: a valid MSG to fill; 0 or -1 ends the loop.
    while unsafe { GetMessageW(&raw mut message, None, 0, 0) }.0 > 0 {}
    // SAFETY: the hook installed above.
    let _ = unsafe { UnhookWindowsHookEx(hook) };
}

/// The low-level mouse hook: notes left-button presses and, when the
/// release is at the same spot, posts the click; posts pointer moves over
/// widgets. Kept tiny: it runs for every mouse event on the system.
unsafe extern "system" fn mouse_hook(
    code: i32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    let message = u32::try_from(wparam.0).unwrap_or(0);
    let watched =
        matches!(message, WM_LBUTTONDOWN | WM_LBUTTONUP | WM_MOUSEMOVE);
    if code >= 0 && watched {
        let info = std::ptr::with_exposed_provenance::<MSLLHOOKSTRUCT>(
            lparam.0.cast_unsigned(),
        );
        // SAFETY: for WH_MOUSE_LL, lParam points to an MSLLHOOKSTRUCT
        // that's valid for the duration of the call.
        if let Some(info) = unsafe { info.as_ref() } {
            if message == WM_MOUSEMOVE {
                note_move(info.pt);
            } else {
                note_button(message, info.pt);
            }
        }
    }
    // SAFETY: passes the event on unchanged, as hooks must.
    unsafe { CallNextHookEx(None, code, wparam, lparam) }
}

/// Remembers a press; on a release at the same spot, posts a click.
fn note_button(message: u32, point: POINT) {
    let Ok(mut shared) = SHARED.lock() else {
        return;
    };
    if message == WM_LBUTTONDOWN {
        shared.pressed = Some(point);
    } else if let Some(down) = shared.pressed.take() {
        let near = down.x.saturating_sub(point.x).abs() <= CLICK_SLOP
            && down.y.saturating_sub(point.y).abs() <= CLICK_SLOP;
        if near {
            shared.clicked = Some(point);
            post(shared.notify, shared.click_message);
        }
    }
}

/// A pointer move: over a widget with hover highlights (or just off
/// one), hands the position to the controller, one message at a time.
/// Anywhere else it does nothing.
fn note_move(point: POINT) {
    let Ok(mut shared) = SHARED.lock() else {
        return;
    };
    let inside = shared.zones.iter().any(|z| {
        z.left <= point.x
            && point.x < z.right
            && z.top <= point.y
            && point.y < z.bottom
    });
    if inside || HOVERING.load(Ordering::Acquire) {
        shared.hover_at = Some(point);
        if !HOVER_PENDING.swap(true, Ordering::AcqRel) {
            post(shared.notify, shared.hover_message);
        }
    }
}

/// Posts `message` to the window at `notify`.
fn post(notify: usize, message: u32) {
    if notify == 0 {
        return;
    }
    let hwnd = HWND(std::ptr::with_exposed_provenance_mut(notify));
    // SAFETY: posting is safe from any thread; plain values. If it
    // fails, that click or move is just missed.
    let _ = unsafe { PostMessageW(Some(hwnd), message, WPARAM(0), LPARAM(0)) };
}
