//! Monitor enumeration.

use windows::Win32::Foundation::{LPARAM, RECT};
use windows::Win32::Graphics::Gdi::{EnumDisplayMonitors, HDC, HMONITOR};
use windows::core::BOOL;

use super::to_rect;
use crate::geometry::Rect;

/// Every monitor's rectangle in desktop coordinates (physical pixels).
/// The primary monitor's top-left is (0, 0); others can be negative.
pub fn list() -> Vec<Rect> {
    let mut found: Vec<Rect> = Vec::new();
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

/// `EnumDisplayMonitors` callback: appends one monitor's rectangle to the
/// `Vec<Rect>` that `data` points to.
unsafe extern "system" fn collect(
    _monitor: HMONITOR,
    _dc: HDC,
    rect: *mut RECT,
    data: LPARAM,
) -> BOOL {
    let found = std::ptr::with_exposed_provenance_mut::<Vec<Rect>>(
        data.0.cast_unsigned(),
    );
    // SAFETY: `data` is the `&mut Vec` from `list`, alive and not
    // otherwise borrowed during the enumeration; `rect` is valid for the
    // duration of the callback, as the API promises.
    let pair = unsafe { (found.as_mut(), rect.as_ref()) };
    if let (Some(found), Some(rect)) = pair {
        found.push(to_rect(*rect));
    }
    true.into()
}
