//! The one place `main.rs` reaches the system: each item here comes from
//! the layer for the system being built (`win32` on Windows, `macos` on
//! macOS, `linux` elsewhere), so every layer offers the same set.
//!
//! Only Windows draws on the desktop so far; the others can take
//! readings and render previews. `docs/PORTING.md` has the rest.

use std::path::PathBuf;

use crate::args::Options;
use crate::color::Rgb;
use crate::monitor::Monitor;
use crate::widgets::{Dashboard, Widget};

#[cfg(not(any(windows, target_os = "macos")))]
pub use crate::linux::Sensors;
#[cfg(target_os = "macos")]
pub use crate::macos::Sensors;
#[cfg(windows)]
pub use crate::win32::sensors::Sensors;

/// Lets `--help`, `--diagnose` and errors print when a release build is
/// started from a terminal (only Windows needs to do anything).
pub fn attach_console() {
    #[cfg(windows)]
    crate::win32::console::attach_parent();
}

/// Every monitor. Empty where monitors can't be listed yet.
pub fn monitors() -> Vec<Monitor> {
    #[cfg(windows)]
    {
        crate::win32::monitors::list()
    }
    #[cfg(not(windows))]
    {
        Vec::new()
    }
}

/// How the desktop's windows are laid out, for `--diagnose`.
pub fn diagnose() -> String {
    #[cfg(windows)]
    {
        crate::win32::app::diagnose()
    }
    #[cfg(not(windows))]
    {
        NOT_YET.to_owned()
    }
}

/// Asks the running copy to quit. False if none was found.
pub fn request_quit() -> bool {
    #[cfg(windows)]
    {
        crate::win32::app::request_quit()
    }
    #[cfg(not(windows))]
    {
        false
    }
}

/// Draws the widgets on the desktop until quit.
///
/// # Errors
///
/// What stopped it, ready to log.
pub fn run(
    options: Options,
    config_path: Option<PathBuf>,
) -> Result<(), String> {
    #[cfg(windows)]
    {
        crate::win32::app::run(options, config_path).map_err(|e| e.to_string())
    }
    #[cfg(not(windows))]
    {
        let _ = (options, config_path);
        Err(NOT_YET.to_owned())
    }
}

/// Renders `widget` as it'd look on a monitor `monitor_height` pixels
/// tall, for `--preview`. Returns width, height and pixels
/// (`0x00RRGGBB`, rows top to bottom).
pub fn preview(
    dashboard: &Dashboard,
    widget: Widget,
    monitor_height: u32,
    background: Rgb,
) -> Option<(u32, u32, Vec<u32>)> {
    #[cfg(windows)]
    {
        crate::win32::paint::preview(
            dashboard,
            widget,
            monitor_height,
            background,
        )
    }
    #[cfg(not(windows))]
    {
        crate::preview::render(dashboard, widget, monitor_height, background)
    }
}

/// What the commands that need the desktop say on systems not ported
/// yet.
#[cfg(not(windows))]
const NOT_YET: &str = "drawing on the desktop isn't supported on this \
system yet; --sensors, --preview, --check and --info work";
