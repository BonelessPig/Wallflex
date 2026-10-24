//! Brings Discord's window to the front. A `discord://` link makes
//! Discord navigate but doesn't restore or raise it. Ported from the
//! user's `win_focus.py`.

use std::collections::HashMap;
use std::path::Path;

use windows::Win32::Foundation::{CloseHandle, HWND, LPARAM};
use windows::Win32::System::Threading::{
    OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
    QueryFullProcessImageNameW,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    KEYBD_EVENT_FLAGS, KEYEVENTF_KEYUP, VK_MENU, keybd_event,
};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GW_OWNER, GetWindow, GetWindowTextLengthW,
    GetWindowThreadProcessId, IsIconic, IsWindowVisible, SW_RESTORE, SW_SHOW,
    SetForegroundWindow, ShowWindow,
};
use windows::core::{BOOL, PWSTR};

/// Discord's executables (stable, PTB, Canary), lower case.
const DISCORD_EXES: [&str; 3] =
    ["discord.exe", "discordptb.exe", "discordcanary.exe"];
/// Longest process path read.
const MAX_PATH_CHARS: usize = 1024;

/// Restores and raises Discord's window. Returns whether it was found.
///
/// Windows only lets the program that got the last input take the
/// foreground; tapping Alt first (no character typed) lifts that lock,
/// the usual workaround.
pub fn focus_discord() -> bool {
    let Some(hwnd) = find_discord_window() else {
        return false;
    };
    // SAFETY: plain window calls on a window found just now. Their
    // results are informational: the window may refuse, and nothing more
    // can be done then.
    unsafe {
        if IsIconic(hwnd).as_bool() {
            let _ = ShowWindow(hwnd, SW_RESTORE);
        } else if !IsWindowVisible(hwnd).as_bool() {
            let _ = ShowWindow(hwnd, SW_SHOW);
        }
        let alt = u8::try_from(VK_MENU.0).unwrap_or_default();
        keybd_event(alt, 0, KEYBD_EVENT_FLAGS(0), 0);
        keybd_event(alt, 0, KEYEVENTF_KEYUP, 0);
        let _ = SetForegroundWindow(hwnd);
    }
    true
}

/// Discord's main window: top-level, titled and unowned, in a Discord
/// process; a visible one wins over a hidden one (closed to the tray).
fn find_discord_window() -> Option<HWND> {
    let mut found: Vec<HWND> = Vec::new();
    let data = LPARAM(
        std::ptr::from_mut(&mut found)
            .expose_provenance()
            .cast_signed(),
    );
    // SAFETY: `collect` only runs during this call and `found` outlives
    // it. The result only says whether enumeration finished.
    let _ = unsafe { EnumWindows(Some(collect), data) };
    let mut exe_by_pid: HashMap<u32, bool> = HashMap::new();
    let discord: Vec<HWND> = found
        .into_iter()
        .filter(|&hwnd| {
            let pid = process_id(hwnd);
            *exe_by_pid.entry(pid).or_insert_with(|| is_discord(pid))
        })
        .collect();
    // SAFETY: only reads window state.
    let visible = discord
        .iter()
        .copied()
        .find(|&h| unsafe { IsWindowVisible(h) }.as_bool());
    visible.or_else(|| discord.first().copied())
}

/// `EnumWindows` callback: keeps top-level, titled, unowned windows.
unsafe extern "system" fn collect(hwnd: HWND, data: LPARAM) -> BOOL {
    let found = std::ptr::with_exposed_provenance_mut::<Vec<HWND>>(
        data.0.cast_unsigned(),
    );
    // SAFETY: only reads window state.
    let candidate = unsafe {
        GetWindow(hwnd, GW_OWNER).is_err() && GetWindowTextLengthW(hwnd) > 0
    };
    // SAFETY: `data` is the `&mut Vec` from `find_discord_window`,
    // alive and not otherwise borrowed during the enumeration.
    if let (true, Some(found)) = (candidate, unsafe { found.as_mut() }) {
        found.push(hwnd);
    }
    true.into()
}

/// The id of the process that owns `hwnd`.
fn process_id(hwnd: HWND) -> u32 {
    let mut pid = 0_u32;
    // SAFETY: `pid` is a valid place for the call to write to.
    let _thread = unsafe { GetWindowThreadProcessId(hwnd, Some(&raw mut pid)) };
    pid
}

/// Whether process `pid` is one of Discord's executables.
fn is_discord(pid: u32) -> bool {
    // SAFETY: plain values; the handle is closed below.
    let Ok(process) =
        (unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) })
    else {
        return false;
    };
    let mut buffer = vec![0_u16; MAX_PATH_CHARS];
    let mut size = u32::try_from(buffer.len()).unwrap_or(0);
    // SAFETY: `buffer` holds `size` characters; `size` gets the length.
    let ok = unsafe {
        QueryFullProcessImageNameW(
            process,
            PROCESS_NAME_WIN32,
            PWSTR(buffer.as_mut_ptr()),
            &raw mut size,
        )
    }
    .is_ok();
    // SAFETY: closes the handle opened above.
    let _ = unsafe { CloseHandle(process) };
    if !ok {
        return false;
    }
    let length = usize::try_from(size).unwrap_or(0);
    let path =
        String::from_utf16_lossy(buffer.get(..length).unwrap_or_default());
    let exe = Path::new(&path)
        .file_name()
        .map(|n| n.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    DISCORD_EXES.contains(&exe.as_str())
}
