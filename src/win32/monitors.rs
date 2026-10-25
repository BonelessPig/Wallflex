//! Monitor enumeration.

use windows::Win32::Foundation::{ERROR_SUCCESS, LPARAM, RECT};
use windows::Win32::Graphics::Gdi::{
    DISPLAY_DEVICE_ACTIVE, DISPLAY_DEVICEW, EnumDisplayDevicesW,
    EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITORINFOEXW,
};
use windows::Win32::System::Registry::{
    HKEY_LOCAL_MACHINE, RRF_RT_REG_BINARY, RegGetValueW,
};
use windows::Win32::UI::WindowsAndMessaging::EDD_GET_DEVICE_INTERFACE_NAME;
use windows::core::{BOOL, HSTRING, PCWSTR, w};

use super::{struct_size, to_rect};
use crate::edid;
use crate::geometry::Rect;

/// `MONITORINFO.dwFlags` bit for the primary monitor.
const MONITORINFOF_PRIMARY: u32 = 1;
/// Screens to look through on one display (mirrored screens share
/// one).
const MAX_SCREENS_PER_DISPLAY: u32 = 8;
/// Room for an EDID with all its extension blocks.
const EDID_MAX: usize = 32 * 1024;

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
    /// The screen's width and height in millimetres, if the monitor
    /// says.
    pub size_mm: Option<(u32, u32)>,
}

impl Monitor {
    /// Millimetres per pixel across, if the size is known.
    pub fn pitch(&self) -> Option<f64> {
        let (width_mm, _) = self.size_mm?;
        let pixels = self.rect.width();
        (pixels > 0).then(|| f64::from(width_mm) / f64::from(pixels))
    }
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
        size_mm: physical_size(device),
    })
}

/// The size in millimetres of the (first active) screen on the display
/// named `device` (null-terminated `\\.\DISPLAYn`), from its EDID.
fn physical_size(device: &[u16]) -> Option<(u32, u32)> {
    let mut screen = DISPLAY_DEVICEW {
        cb: struct_size::<DISPLAY_DEVICEW>(),
        ..Default::default()
    };
    let found = (0..MAX_SCREENS_PER_DISPLAY).any(|index| {
        // SAFETY: `device` is null-terminated (from MONITORINFOEXW) and
        // `screen` is a DISPLAY_DEVICEW with its size set.
        let ok = unsafe {
            EnumDisplayDevicesW(
                PCWSTR(device.as_ptr()),
                index,
                &raw mut screen,
                EDD_GET_DEVICE_INTERFACE_NAME,
            )
        };
        ok.as_bool() && (screen.StateFlags & DISPLAY_DEVICE_ACTIVE).0 != 0
    });
    if !found {
        return None;
    }
    let id = &screen.DeviceID;
    let end = id.iter().position(|&c| c == 0).unwrap_or(id.len());
    let interface = String::from_utf16_lossy(id.get(..end)?);
    let key = edid::registry_key(&interface)?;
    edid::size_mm(&read_edid(&key)?)
}

/// The `EDID` value under `key` in `HKEY_LOCAL_MACHINE`.
fn read_edid(key: &str) -> Option<Vec<u8>> {
    let key = HSTRING::from(key);
    let mut data = vec![0_u8; EDID_MAX];
    let mut size = u32::try_from(data.len()).ok()?;
    // SAFETY: the strings outlive the call, and `data` holds the `size`
    // bytes the call may write.
    let status = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            &key,
            w!("EDID"),
            RRF_RT_REG_BINARY,
            None,
            Some(data.as_mut_ptr().cast()),
            Some(&raw mut size),
        )
    };
    if status != ERROR_SUCCESS {
        return None;
    }
    data.truncate(usize::try_from(size).ok()?);
    Some(data)
}
