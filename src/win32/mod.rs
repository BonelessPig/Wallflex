//! The Win32 layer. This is the only module tree allowed to use `unsafe`,
//! and every `unsafe` block in it says why it is sound.

#![allow(unsafe_code)]

pub mod app;
pub mod console;
mod desktop;
mod discord;
mod http;
pub mod images;
mod monitors;
pub mod paint;
pub mod sensors;
mod startup;
mod surface;
mod tray;

use windows::Win32::Foundation::{HWND, RECT};
use windows::Win32::UI::WindowsAndMessaging::GetClassNameW;

use crate::geometry::Rect;

/// Longest window class name Win32 allows, plus the terminator.
const MAX_CLASS_NAME: usize = 257;

/// Converts a Win32 `RECT` into a [`Rect`].
fn to_rect(r: RECT) -> Rect {
    Rect {
        left: r.left,
        top: r.top,
        right: r.right,
        bottom: r.bottom,
    }
}

/// Converts a [`Rect`] into a Win32 `RECT`.
fn to_win32_rect(r: Rect) -> RECT {
    RECT {
        left: r.left,
        top: r.top,
        right: r.right,
        bottom: r.bottom,
    }
}

/// Size of a Win32 struct, for its `cbSize` field.
fn struct_size<T>() -> u32 {
    // Win32 structs are far smaller than 4 GB; the fallback never runs.
    u32::try_from(size_of::<T>()).unwrap_or(u32::MAX)
}

/// The window's class name, or an empty string if it can't be read.
fn class_name(hwnd: HWND) -> String {
    let mut buffer = [0_u16; MAX_CLASS_NAME];
    // SAFETY: the buffer is valid for its whole length, which the call
    // receives through the slice.
    let written = unsafe { GetClassNameW(hwnd, &mut buffer) };
    usize::try_from(written)
        .ok()
        .and_then(|len| buffer.get(..len))
        .map(String::from_utf16_lossy)
        .unwrap_or_default()
}
