//! Console helpers.

use windows::Win32::System::Console::{ATTACH_PARENT_PROCESS, AttachConsole};

/// Attaches to the console of the process that started Wallflex, if any,
/// so a release build started from a terminal can print. Does nothing
/// when there is no parent console or one is already attached.
pub fn attach_parent() {
    // SAFETY: takes no pointers. Failure only means there is no console to
    // attach to, which is fine, so the result is deliberately ignored.
    let _ = unsafe { AttachConsole(ATTACH_PARENT_PROCESS) };
}
