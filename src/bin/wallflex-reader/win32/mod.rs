//! The reader's Win32 side: one borderless window holding a `WebView2`
//! control, shown where the book icon was clicked.
//!
//! Only one copy runs. A second start (another click on the book icon)
//! hands its `--at` point to the first copy with `WM_COPYDATA` and
//! exits; the first then closes. Closing ends the process, so the
//! browser's memory goes back to Windows. A start while a closing copy
//! is still finishing waits for it to go, then opens.
//!
//! This module (and its children) may use `unsafe`, like Wallflex's
//! own `src/win32`; every block says why it's sound.
#![allow(unsafe_code)]

mod webview;
mod window;

use std::cell::RefCell;
use std::path::PathBuf;
use std::time::Duration;

use windows::Win32::Foundation::{
    CloseHandle, ERROR_ALREADY_EXISTS, GetLastError, HANDLE, HWND, LPARAM,
    LRESULT, POINT, WPARAM,
};
use windows::Win32::System::Com::{
    COINIT_APARTMENTTHREADED, CoInitializeEx, CoUninitialize,
};
use windows::Win32::System::DataExchange::COPYDATASTRUCT;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::CreateMutexW;
use windows::Win32::UI::Shell::ShellExecuteW;
use windows::Win32::UI::WindowsAndMessaging::{
    DefWindowProcW, DestroyWindow, DispatchMessageW, FindWindowW, GetCursorPos,
    GetMessageW, MB_ICONINFORMATION, MB_OK, MSG, MessageBoxW, SW_SHOWNORMAL,
    SendMessageW, TranslateMessage, WM_COPYDATA,
};
use windows::core::{HSTRING, PCWSTR, w};

use crate::messages::Request;
use window::Reader;

/// The window class; also how a second copy finds the first. Debug
/// builds use their own names, so a test copy never reaches (or toggles)
/// the installed reader.
#[cfg(not(debug_assertions))]
const CLASS: PCWSTR = w!("WallflexReader");
/// See the release build's `CLASS`.
#[cfg(debug_assertions)]
const CLASS: PCWSTR = w!("WallflexReaderDev");
/// Held while a copy runs.
#[cfg(not(debug_assertions))]
const MUTEX: PCWSTR = w!("Local\\WallflexReader");
/// See the release build's `MUTEX`.
#[cfg(debug_assertions)]
const MUTEX: PCWSTR = w!("Local\\WallflexReaderDev");
/// `WM_COPYDATA` tag for a point to open at (`X,Y` in UTF-8).
const COPY_AT: usize = 0x5752_4154;
/// `WM_COPYDATA` tag asking the running copy to exit.
const COPY_QUIT: usize = 0x5752_5155;
/// How many times a second copy looks for the first copy's window
/// (which may still be starting), or for a closing copy to finish.
const FIND_TRIES: u32 = 30;
/// The wait between those looks.
const FIND_WAIT: Duration = Duration::from_millis(100);
/// Where to get `WebView2` if it's missing.
const WEBVIEW2_URL: &str =
    "https://developer.microsoft.com/microsoft-edge/webview2/";

/// What the reader needs to start.
pub struct Launch {
    /// Where the book icon was clicked; the pointer if not given.
    pub at: Option<(i32, i32)>,
    /// The guides folder.
    pub folder: PathBuf,
    /// Where `reader.json` is.
    pub saved: PathBuf,
    /// Where `WebView2` keeps its data.
    pub data: PathBuf,
}

/// Something the page or the browser reports.
pub enum Event {
    /// A request from the page.
    Request(Request),
    /// A link to a guide was followed.
    Open(String),
    /// A web page link was followed: show this address in reader mode.
    OpenWeb(String),
    /// A web page's back link was followed.
    WebBack,
    /// A web page's save link was followed.
    WebSave,
    /// Esc was pressed.
    Escape,
    /// The page's zoom changed (Ctrl and the mouse wheel, or the app).
    Zoomed(f64),
}

thread_local! {
    /// The reader, once its window and browser exist. UI thread only.
    static READER: RefCell<Option<Reader>> = const { RefCell::new(None) };
}

/// Runs the reader until it exits, or hands `launch.at` to the copy
/// that's already running.
///
/// # Errors
///
/// What stopped it starting.
pub fn run(launch: &Launch) -> Result<(), String> {
    // The handle is kept until the process ends, which releases it.
    let Some(_mutex) = claim(launch.at)? else {
        return Ok(());
    };
    // SAFETY: once per thread, before any COM use; balanced below.
    unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) }
        .ok()
        .map_err(|e| e.message())?;
    let result = start(launch);
    if let Err(error) = &result {
        let text = format!(
            "Wallflex's guides reader couldn't start: {error}\n\nIt needs \
             Microsoft Edge WebView2, which comes with Windows 11. If it's \
             missing, get it from {WEBVIEW2_URL}"
        );
        // SAFETY: the strings outlive the call.
        let _ = unsafe {
            MessageBoxW(
                None,
                &HSTRING::from(text),
                w!("Wallflex guides"),
                MB_OK | MB_ICONINFORMATION,
            )
        };
    }
    if result.is_ok() {
        message_loop();
    }
    if let Some(reader) = READER.with(|r| r.borrow_mut().take()) {
        reader.close();
    }
    // SAFETY: balances `CoInitializeEx`, after every COM object is gone.
    unsafe { CoUninitialize() };
    result
}

/// Creates the window and browser and shows them at the launch point.
fn start(launch: &Launch) -> Result<(), String> {
    // SAFETY: plain call; `None` asks for this exe's module.
    let module = unsafe { GetModuleHandleW(None) }.map_err(|e| e.message())?;
    let mut reader =
        Reader::create(module.into(), CLASS, Some(window_proc), launch)?;
    reader.show(launch.at.unwrap_or_else(pointer));
    READER.with(|r| *r.borrow_mut() = Some(reader));
    Ok(())
}

/// Takes the one-copy lock, or hands `at` to the copy holding it
/// (`None`: handed over). A copy that's closing holds the lock a moment
/// after its window has gone; then this waits for it to finish.
fn claim(at: Option<(i32, i32)>) -> Result<Option<HANDLE>, String> {
    let (x, y) = at.unwrap_or_else(pointer);
    let text = format!("{x},{y}");
    for _ in 0..FIND_TRIES {
        // SAFETY: plain values; the name is a static string.
        let mutex = unsafe { CreateMutexW(None, false, MUTEX) };
        // SAFETY: reads the calling thread's last error, set just above.
        let already = unsafe { GetLastError() } == ERROR_ALREADY_EXISTS;
        let mutex = mutex.map_err(|e| e.message())?;
        if !already {
            return Ok(Some(mutex));
        }
        // SAFETY: our own handle, not used again. Holding it would keep
        // the lock alive after the other copy has gone; if it can't be
        // closed, this start just ends up handing over or giving up.
        let _ = unsafe { CloseHandle(mutex) };
        if send_point(&text)? {
            return Ok(None);
        }
        std::thread::sleep(FIND_WAIT);
    }
    Err("the running reader didn't answer".to_owned())
}

/// Hands `text` (the point, `X,Y`) to the running copy's window. False
/// if there's no window (still starting, or already gone) or the copy
/// is ending and didn't take it.
fn send_point(text: &str) -> Result<bool, String> {
    // SAFETY: the class name is a static string.
    let Ok(hwnd) = (unsafe { FindWindowW(CLASS, PCWSTR::null()) }) else {
        return Ok(false);
    };
    let data = COPYDATASTRUCT {
        dwData: COPY_AT,
        cbData: u32::try_from(text.len()).map_err(|e| e.to_string())?,
        lpData: text.as_ptr().cast_mut().cast(),
    };
    let address = std::ptr::from_ref(&data).expose_provenance();
    // SAFETY: `data` and the text it points to outlive the call, which
    // returns once the window has read them.
    let taken = unsafe {
        SendMessageW(
            hwnd,
            WM_COPYDATA,
            Some(WPARAM(0)),
            Some(LPARAM(address.cast_signed())),
        )
    };
    Ok(taken.0 != 0)
}

/// Asks the running copy to exit. False if none is running.
pub fn quit() -> bool {
    // SAFETY: the class name is a static string.
    let Ok(hwnd) = (unsafe { FindWindowW(CLASS, PCWSTR::null()) }) else {
        return false;
    };
    let data = COPYDATASTRUCT {
        dwData: COPY_QUIT,
        cbData: 0,
        lpData: std::ptr::null_mut(),
    };
    let address = std::ptr::from_ref(&data).expose_provenance();
    // SAFETY: `data` outlives the call, which returns once the window
    // has handled it.
    unsafe {
        SendMessageW(
            hwnd,
            WM_COPYDATA,
            Some(WPARAM(0)),
            Some(LPARAM(address.cast_signed())),
        );
    }
    true
}

/// Where the pointer is, in desktop pixels; (0, 0) if unknown.
fn pointer() -> (i32, i32) {
    let mut point = POINT::default();
    // SAFETY: `point` is a valid place for the call to write. On failure
    // it stays (0, 0), which places the window on the main monitor.
    let _ = unsafe { GetCursorPos(&raw mut point) };
    (point.x, point.y)
}

/// Runs `f` on the reader, unless it's missing or already in use (a
/// message sent while it's busy is then dropped).
fn with_reader(f: impl FnOnce(&mut Reader)) {
    READER.with(|reader| {
        if let Ok(mut reader) = reader.try_borrow_mut()
            && let Some(reader) = reader.as_mut()
        {
            f(reader);
        }
    });
}

/// Passes on what the page or browser reported.
fn handle(event: Event) {
    with_reader(|reader| reader.on_event(event));
}

/// Opens a web or mail link in its usual app.
fn open_external(url: &str) {
    shell_open(&HSTRING::from(url));
}

/// Opens a link, file or folder the way Explorer would.
fn shell_open(target: &HSTRING) {
    // SAFETY: the strings outlive the call; the result only says whether
    // it worked, and there's nothing to do if it didn't.
    let _ = unsafe {
        ShellExecuteW(
            None,
            w!("open"),
            target,
            PCWSTR::null(),
            PCWSTR::null(),
            SW_SHOWNORMAL,
        )
    };
}

/// Pumps messages until `WM_QUIT`.
fn message_loop() {
    let mut msg = MSG::default();
    // SAFETY: `msg` is a valid place for each call to use.
    while unsafe { GetMessageW(&raw mut msg, None, 0, 0) }.as_bool() {
        // SAFETY: as above.
        unsafe {
            let _ = TranslateMessage(&raw const msg);
            DispatchMessageW(&raw const msg);
        }
    }
}

/// The window's message handler.
extern "system" fn window_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if msg == WM_COPYDATA {
        if is_quit(lparam) {
            // Saves where the reader was, as any close does.
            with_reader(|reader| reader.quit(false));
            // SAFETY: our own window; WM_DESTROY then ends the loop.
            let _ = unsafe { DestroyWindow(hwnd) };
            return LRESULT(1);
        }
        if copied_point(lparam).is_some() {
            // Taken (1) unless the reader is ending; then the other copy
            // waits for it to go and opens instead.
            let mut taken = true;
            with_reader(|reader| taken = reader.clicked_again());
            return LRESULT(isize::from(taken));
        }
        return LRESULT(1);
    }
    if let Some(result) = window::handle_message(hwnd, msg, wparam, lparam) {
        return result;
    }
    // SAFETY: passes the message on unchanged.
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}

/// Whether the `WM_COPYDATA` in `lparam` is the request to exit.
fn is_quit(lparam: LPARAM) -> bool {
    let data = std::ptr::with_exposed_provenance::<COPYDATASTRUCT>(
        lparam.0.cast_unsigned(),
    );
    // SAFETY: for WM_COPYDATA, lParam points to a COPYDATASTRUCT that's
    // valid while the message is handled.
    unsafe { data.as_ref() }.is_some_and(|d| d.dwData == COPY_QUIT)
}

/// The point a second copy sent with `WM_COPYDATA`, if that's what it is.
fn copied_point(lparam: LPARAM) -> Option<(i32, i32)> {
    let data = std::ptr::with_exposed_provenance::<COPYDATASTRUCT>(
        lparam.0.cast_unsigned(),
    );
    // SAFETY: for WM_COPYDATA, lParam points to a COPYDATASTRUCT that's
    // valid while the message is handled.
    let data = unsafe { data.as_ref() }?;
    if data.dwData != COPY_AT || data.lpData.is_null() {
        return None;
    }
    let length = usize::try_from(data.cbData).ok()?;
    // SAFETY: the sender's buffer holds `cbData` bytes, copied into our
    // process by Windows and valid while the message is handled.
    let bytes =
        unsafe { std::slice::from_raw_parts(data.lpData.cast::<u8>(), length) };
    crate::args::parse_point(std::str::from_utf8(bytes).ok()?)
}
