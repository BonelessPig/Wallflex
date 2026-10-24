//! The notification-area (tray) icon and its menu: open the settings
//! or the log, start with Windows, quit. The icon is the app icon
//! embedded in the exe (see `src/icon.rs`).

use windows::Win32::Foundation::{HWND, LPARAM, POINT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Shell::{
    NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NOTIFYICONDATAW,
    Shell_NotifyIconW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreatePopupMenu, DestroyIcon, DestroyMenu, GetCursorPos,
    GetSystemMetrics, HICON, IMAGE_ICON, LR_DEFAULTCOLOR, LoadImageW,
    MF_CHECKED, MF_SEPARATOR, MF_STRING, MF_UNCHECKED, PostMessageW,
    SM_CXSMICON, SetForegroundWindow, TPM_NONOTIFY, TPM_RETURNCMD,
    TPM_RIGHTBUTTON, TrackPopupMenu, WM_NULL,
};
use windows::core::{PCWSTR, w};

use super::struct_size;
use crate::log;

/// The icon's id among this window's tray icons.
const TRAY_ID: u32 = 1;
/// Hover text.
const TIP: &str = "Wallflex";
/// Fallback icon size if the system doesn't say.
const DEFAULT_ICON: i32 = 16;
/// The app icon's resource id (`ICON_ID` in `build.rs`).
const ICON_ID: usize = 1;

/// What was picked in the tray menu.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Choice {
    /// Open the settings file.
    Settings,
    /// Open the log.
    Log,
    /// Turn starting with Windows on or off.
    Startup,
    /// Quit Wallflex.
    Quit,
}

impl Choice {
    /// Every choice, with its menu id.
    const ALL: [(Self, usize); 4] = [
        (Self::Settings, 1),
        (Self::Log, 2),
        (Self::Startup, 3),
        (Self::Quit, 4),
    ];

    /// The menu id.
    fn id(self) -> usize {
        Self::ALL
            .iter()
            .find(|(c, _)| *c == self)
            .map_or(0, |(_, id)| *id)
    }
}

/// The tray icon; removed on drop.
pub struct Tray {
    /// The window that gets its messages.
    hwnd: HWND,
    /// The message it sends.
    message: u32,
    /// The icon, destroyed on drop.
    icon: HICON,
}

impl Tray {
    /// Adds the icon. Clicks arrive as `message` on `hwnd`, with the
    /// mouse message in `lParam`.
    pub fn add(hwnd: HWND, message: u32) -> Option<Self> {
        let Some(icon) = load_icon() else {
            log!("can't load the tray icon");
            return None;
        };
        let tray = Self {
            hwnd,
            message,
            icon,
        };
        tray.show();
        Some(tray)
    }

    /// Puts the icon (back) in the notification area, as after Explorer
    /// restarts.
    pub fn show(&self) {
        let data = self.data();
        // SAFETY: `data` is a filled NOTIFYICONDATAW with its size set.
        if !unsafe { Shell_NotifyIconW(NIM_ADD, &raw const data) }.as_bool() {
            log!("can't add the tray icon");
        }
    }

    /// The icon's description for `Shell_NotifyIconW`.
    fn data(&self) -> NOTIFYICONDATAW {
        let mut data = NOTIFYICONDATAW {
            cbSize: struct_size::<NOTIFYICONDATAW>(),
            hWnd: self.hwnd,
            uID: TRAY_ID,
            uFlags: NIF_ICON | NIF_MESSAGE | NIF_TIP,
            uCallbackMessage: self.message,
            hIcon: self.icon,
            ..Default::default()
        };
        for (slot, unit) in data.szTip.iter_mut().zip(TIP.encode_utf16()) {
            *slot = unit;
        }
        data
    }
}

impl Drop for Tray {
    fn drop(&mut self) {
        let data = self.data();
        // SAFETY: removes the icon added in `add`, then destroys the
        // icon handle it used. Failures can't be handled.
        unsafe {
            let _ = Shell_NotifyIconW(NIM_DELETE, &raw const data);
            let _ = DestroyIcon(self.icon);
        }
    }
}

/// Shows the tray menu at the pointer and returns the choice, if any.
/// `startup` says whether "Start with Windows" is ticked.
pub fn show_menu(hwnd: HWND, startup: bool) -> Option<Choice> {
    // SAFETY: a new, empty menu, destroyed below.
    let menu = unsafe { CreatePopupMenu() }.ok()?;
    let tick = if startup { MF_CHECKED } else { MF_UNCHECKED };
    let items: [(Option<Choice>, PCWSTR); 5] = [
        (Some(Choice::Settings), w!("Open settings")),
        (Some(Choice::Log), w!("Open log")),
        (Some(Choice::Startup), w!("Start with Windows")),
        (None, PCWSTR::null()),
        (Some(Choice::Quit), w!("Quit Wallflex")),
    ];
    for (choice, label) in items {
        let flags = match choice {
            Some(Choice::Startup) => MF_STRING | tick,
            Some(_) => MF_STRING,
            None => MF_SEPARATOR,
        };
        let id = choice.map_or(0, Choice::id);
        // SAFETY: the menu is valid and the labels are constants.
        if let Err(error) = unsafe { AppendMenuW(menu, flags, id, label) } {
            log!("tray menu: {error}");
        }
    }
    let mut point = POINT::default();
    // SAFETY: `point` is a valid place to write to.
    let _ = unsafe { GetCursorPos(&raw mut point) };
    // SAFETY: the window and menu are valid. The foreground switch lets
    // the menu close when you click elsewhere; the WM_NULL afterwards is
    // the documented fix for menus that won't close. Results are
    // informational.
    let picked = unsafe {
        let _ = SetForegroundWindow(hwnd);
        let picked = TrackPopupMenu(
            menu,
            TPM_RETURNCMD | TPM_RIGHTBUTTON | TPM_NONOTIFY,
            point.x,
            point.y,
            None,
            hwnd,
            None,
        );
        let _ = PostMessageW(Some(hwnd), WM_NULL, WPARAM(0), LPARAM(0));
        let _ = DestroyMenu(menu);
        picked
    };
    let id = usize::try_from(picked.0).ok()?;
    Choice::ALL
        .iter()
        .find(|(_, choice_id)| *choice_id == id)
        .map(|(choice, _)| *choice)
}

/// Loads the app icon (`wallflex.ico`, embedded by `build.rs`) at the
/// system's small-icon size; Windows picks the closest image in it.
fn load_icon() -> Option<HICON> {
    // SAFETY: plain query.
    let size = match unsafe { GetSystemMetrics(SM_CXSMICON) } {
        s if s > 0 => s,
        _ => DEFAULT_ICON,
    };
    // SAFETY: a null name asks for this exe's own module.
    let module = unsafe { GetModuleHandleW(None) }.ok()?;
    // A resource id goes in the name pointer's low bits, as
    // MAKEINTRESOURCE does; it is never dereferenced as a string.
    let name = PCWSTR(std::ptr::without_provenance(ICON_ID));
    // SAFETY: `module` is this exe and `name` an integer resource id.
    // Without LR_SHARED the icon is ours to destroy, as `Tray` does.
    let image = unsafe {
        LoadImageW(
            Some(module.into()),
            name,
            IMAGE_ICON,
            size,
            size,
            LR_DEFAULTCOLOR,
        )
    }
    .ok()?;
    Some(HICON(image.0))
}
