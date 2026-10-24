//! Starting with Windows: a value under the current user's `Run` key,
//! which needs no admin rights.

use windows::Win32::Foundation::ERROR_SUCCESS;
use windows::Win32::System::Registry::{
    HKEY_CURRENT_USER, REG_SZ, RRF_RT_REG_SZ, RegDeleteKeyValueW, RegGetValueW,
    RegSetKeyValueW,
};
use windows::core::{PCWSTR, w};

/// Programs listed here start when the user signs in.
const RUN_KEY: PCWSTR = w!(r"Software\Microsoft\Windows\CurrentVersion\Run");
/// Wallflex's value under it.
const VALUE: PCWSTR = w!("Wallflex");

/// Whether Wallflex starts with Windows.
pub fn is_enabled() -> bool {
    // SAFETY: the strings are constants; only the value's existence is
    // asked for, so no buffer is passed.
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            RUN_KEY,
            VALUE,
            RRF_RT_REG_SZ,
            None,
            None,
            None,
        )
    };
    status == ERROR_SUCCESS
}

/// Turns starting with Windows on (for this exe) or off.
///
/// # Errors
///
/// A message when the registry can't be changed.
pub fn set_enabled(enabled: bool) -> Result<(), String> {
    if !enabled {
        // SAFETY: the strings are constants.
        let status =
            unsafe { RegDeleteKeyValueW(HKEY_CURRENT_USER, RUN_KEY, VALUE) };
        return status.ok().map_err(|e| e.message());
    }
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    // Quoted, so a path with spaces still works.
    let command = format!("\"{}\"", exe.display());
    let wide: Vec<u16> =
        command.encode_utf16().chain(std::iter::once(0)).collect();
    let bytes: Vec<u8> =
        wide.iter().flat_map(|unit| unit.to_le_bytes()).collect();
    let length = u32::try_from(bytes.len()).map_err(|e| e.to_string())?;
    // SAFETY: the strings are constants and `bytes` holds `length` bytes
    // of a null-terminated UTF-16 string, as REG_SZ expects.
    let status = unsafe {
        RegSetKeyValueW(
            HKEY_CURRENT_USER,
            RUN_KEY,
            VALUE,
            REG_SZ.0,
            Some(bytes.as_ptr().cast()),
            length,
        )
    };
    status.ok().map_err(|e| e.message())
}
