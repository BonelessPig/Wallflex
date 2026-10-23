//! Monitor enumeration.

use windows::Win32::Foundation::{LPARAM, RECT};
use windows::Win32::Graphics::Gdi::{
    EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITORINFOEXW,
};
use windows::core::BOOL;

use super::{struct_size, to_rect};
use crate::geometry::Rect;

/// `MONITORINFO.dwFlags` bit for the primary monitor.
const MONITORINFOF_PRIMARY: u32 = 1;

/// One monitor, in desktop coordinates (physical pixels). The primary
/// monitor's top-left is (0, 0); others can be negative.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Monitor {
    /// The whole monitor.
    pub rect: Rect,
    /// The part the taskbar and docked toolbars leave free.
    pub work: Rect,
    /// Windows name, e.g. `\\.\DISPLAY1`.
    pub name: String,
    /// Whether it's the primary monitor.
    pub primary: bool,
}

/// Every monitor.
pub fn list() -> Vec<Monitor> {
    let mut found: Vec<Monitor> = Vec::new();
    let data = LPARAM(
        std::ptr::from_mut(&mut found)
            .expose_provenance()
            .cast_signed(),
    );
    // SAFETY: `collect` only runs during this call, and `found`, which
    // `data` points to, outlives it. The result only says whether the
    // enumeration ran; an empty list covers the failure case.
    let _ = unsafe { EnumDisplayMonitors(None, None, Some(collect), data) };
    found
}

/// `EnumDisplayMonitors` callback: appends one monitor to the
/// `Vec<Monitor>` that `data` points to.
unsafe extern "system" fn collect(
    monitor: HMONITOR,
    _dc: HDC,
    _rect: *mut RECT,
    data: LPARAM,
) -> BOOL {
    let found = std::ptr::with_exposed_provenance_mut::<Vec<Monitor>>(
        data.0.cast_unsigned(),
    );
    // SAFETY: `data` is the `&mut Vec` from `list`, alive and not
    // otherwise borrowed during the enumeration.
    let found = unsafe { found.as_mut() };
    if let (Some(found), Some(info)) = (found, details(monitor)) {
        found.push(info);
    }
    true.into()
}

/// A monitor's rectangle, work area, name and primary flag.
fn details(monitor: HMONITOR) -> Option<Monitor> {
    let mut info = MONITORINFOEXW::default();
    info.monitorInfo.cbSize = struct_size::<MONITORINFOEXW>();
    // SAFETY: `info` is a MONITORINFOEXW with its size set, which the
    // call accepts in place of a MONITORINFO.
    let ok = unsafe {
        GetMonitorInfoW(monitor, std::ptr::from_mut(&mut info).cast())
    };
    if !ok.as_bool() {
        return None;
    }
    let device = &info.szDevice;
    let end = device.iter().position(|&c| c == 0).unwrap_or(device.len());
    Some(Monitor {
        rect: to_rect(info.monitorInfo.rcMonitor),
        work: to_rect(info.monitorInfo.rcWork),
        name: String::from_utf16_lossy(device.get(..end).unwrap_or_default()),
        primary: info.monitorInfo.dwFlags & MONITORINFOF_PRIMARY != 0,
    })
}
