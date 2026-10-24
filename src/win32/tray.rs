//! The notification-area (tray) icon and its menu: open the settings
//! or the log, start with Windows, quit. The icon is drawn at startup
//! with the canvas, at the system's small-icon size, so no image file
//! is needed.

use windows::Win32::Foundation::{HWND, LPARAM, POINT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, CreateBitmap, CreateDIBSection,
    DIB_RGB_COLORS, DeleteObject, GdiFlush, HBITMAP,
};
use windows::Win32::UI::Shell::{
    NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NOTIFYICONDATAW,
    Shell_NotifyIconW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreateIconIndirect, CreatePopupMenu, DestroyIcon, DestroyMenu,
    GetCursorPos, GetSystemMetrics, HICON, ICONINFO, MF_CHECKED, MF_SEPARATOR,
    MF_STRING, MF_UNCHECKED, PostMessageW, SM_CXSMICON, SetForegroundWindow,
    TPM_NONOTIFY, TPM_RETURNCMD, TPM_RIGHTBUTTON, TrackPopupMenu, WM_NULL,
};
use windows::core::{PCWSTR, w};

use super::struct_size;
use crate::canvas::{Canvas, Paint};
use crate::color::Rgb;
use crate::log;

/// The icon's id among this window's tray icons.
const TRAY_ID: u32 = 1;
/// Hover text.
const TIP: &str = "Wallflex";
/// Fallback icon size if the system doesn't say.
const DEFAULT_ICON: i32 = 16;
/// The icon's background (the widgets' blue).
const ICON_BLUE: Rgb = Rgb {
    r: 0xA8,
    g: 0xE6,
    b: 0xFF,
};
/// The icon's bars.
const ICON_DARK: Rgb = Rgb {
    r: 0x10,
    g: 0x14,
    b: 0x18,
};

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
        let icon = draw_icon()?;
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

/// Draws the icon: three bars, like the CPU columns, on a blue rounded
/// square.
fn draw_icon() -> Option<HICON> {
    // SAFETY: plain query.
    let size = match unsafe { GetSystemMetrics(SM_CXSMICON) } {
        s if s > 0 => s,
        _ => DEFAULT_ICON,
    };
    let edge = usize::try_from(size).ok()?;
    let count = edge.checked_mul(edge)?;
    let s = f64::from(size);
    let half = s / 2.0;
    // The shape's coverage, for the alpha channel.
    let mut mask = vec![0_u32; count];
    let white = Paint {
        color: Rgb {
            r: 255,
            g: 255,
            b: 255,
        },
        alpha: 1.0,
    };
    Canvas::new(edge, edge, &mut mask)?.fill_rounded(
        (half, half),
        half,
        s * 0.22,
        white,
    );
    // The picture itself.
    let mut art = vec![0_u32; count];
    {
        let mut canvas = Canvas::new(edge, edge, &mut art)?;
        canvas.fill(ICON_BLUE);
        let bar = s * 0.16;
        let heights = [0.45, 0.72, 0.58];
        let dark = Paint {
            color: ICON_DARK,
            alpha: 1.0,
        };
        for (index, height) in (0_u32..).zip(heights) {
            let x0 = s * 0.22 + f64::from(index) * (bar + s * 0.06);
            let area = crate::canvas::Area {
                x0,
                y0: s * (0.82 - height),
                x1: x0 + bar,
                y1: s * 0.82,
            };
            canvas.fill_rect(area, dark);
        }
    }
    let pixels: Vec<u32> = art
        .iter()
        .zip(&mask)
        .map(|(colour, coverage)| {
            let [b, g, r, _] = colour.to_le_bytes();
            let [alpha, _, _, _] = coverage.to_le_bytes();
            u32::from_le_bytes([b, g, r, alpha])
        })
        .collect();
    make_icon(size, &pixels)
}

/// Builds an icon from straight-alpha BGRA pixels, rows top to bottom.
fn make_icon(size: i32, pixels: &[u32]) -> Option<HICON> {
    let info = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: struct_size::<BITMAPINFOHEADER>(),
            biWidth: size,
            biHeight: size.checked_neg()?,
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        },
        ..Default::default()
    };
    let mut bits = std::ptr::null_mut();
    // SAFETY: `info` describes a size x size 32-bit bitmap; `bits`
    // receives its pixels' address.
    let colour = unsafe {
        CreateDIBSection(
            None,
            &raw const info,
            DIB_RGB_COLORS,
            &raw mut bits,
            None,
            0,
        )
    }
    .ok()?;
    let colour = Bitmap(colour);
    if bits.is_null() {
        return None;
    }
    // SAFETY: `bits` points to size x size u32 pixels, the same count as
    // `pixels`, and nothing else uses them yet.
    unsafe {
        let _ = GdiFlush();
        std::ptr::copy_nonoverlapping(
            pixels.as_ptr(),
            bits.cast::<u32>(),
            pixels.len(),
        );
    }
    // An all-zero mask: the colour bitmap's alpha decides.
    let mask_bytes = vec![
        0_u8;
        usize::try_from(size)
            .ok()?
            .checked_mul(usize::try_from(size).ok()?)?
    ];
    // SAFETY: a monochrome bitmap from enough zero bytes (a bit each).
    let mask = Bitmap(unsafe {
        CreateBitmap(size, size, 1, 1, Some(mask_bytes.as_ptr().cast()))
    });
    let icon_info = ICONINFO {
        fIcon: true.into(),
        xHotspot: 0,
        yHotspot: 0,
        hbmMask: mask.0,
        hbmColor: colour.0,
    };
    // SAFETY: both bitmaps are valid; the icon copies them.
    unsafe { CreateIconIndirect(&raw const icon_info) }.ok()
}

/// A bitmap, deleted on drop.
struct Bitmap(HBITMAP);

impl Drop for Bitmap {
    fn drop(&mut self) {
        // SAFETY: the bitmap is ours and not selected into any DC.
        let _ = unsafe { DeleteObject(self.0.into()) };
    }
}
