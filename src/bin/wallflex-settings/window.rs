//! Opens the settings page: in an Edge app window (no tabs or address
//! bar, so it feels like an app) or, failing that, the default browser.

use std::ffi::OsStr;
use std::path::PathBuf;
use std::process::Command;

/// Edge's exe, under each of the folders it may be installed in.
const EDGE: &str = r"Microsoft\Edge\Application\msedge.exe";
/// Environment variables naming those folders.
const EDGE_ROOTS: [&str; 3] =
    ["ProgramFiles(x86)", "ProgramFiles", "LOCALAPPDATA"];
/// The window's starting size, in pixels.
const WINDOW_SIZE: &str = "--window-size=1360,900";

/// Opens `url`, in an app window unless `browser` asks for the default
/// browser. Problems are printed, since there's nothing else to do.
pub fn open(url: &str, browser: bool) {
    if !browser && let Some(edge) = find_edge() {
        let app = format!("--app={url}");
        match Command::new(&edge)
            .args([app.as_str(), WINDOW_SIZE])
            .spawn()
        {
            Ok(_child) => return,
            Err(error) => eprintln!("can't start {}: {error}", edge.display()),
        }
    }
    if let Err(error) = open_with_system(OsStr::new(url)) {
        eprintln!("can't open the browser: {error}");
    }
}

/// Where Edge is installed, if it is.
fn find_edge() -> Option<PathBuf> {
    EDGE_ROOTS
        .iter()
        .filter_map(std::env::var_os)
        .map(|root| PathBuf::from(root).join(EDGE))
        .find(|path| path.is_file())
}

/// The program that opens a file, folder or URL the way a double-click
/// would: Explorer (which hands URLs to the default browser), `open` on
/// macOS, `xdg-open` elsewhere.
#[cfg(windows)]
const SYSTEM_OPENER: &str = "explorer.exe";
/// See the Windows version.
#[cfg(target_os = "macos")]
const SYSTEM_OPENER: &str = "open";
/// See the Windows version.
#[cfg(not(any(windows, target_os = "macos")))]
const SYSTEM_OPENER: &str = "xdg-open";

/// Opens `target` (a file, folder or URL) with the system's default
/// app for it, without waiting.
///
/// # Errors
///
/// If the opener couldn't be started.
pub fn open_with_system(target: &OsStr) -> std::io::Result<()> {
    Command::new(SYSTEM_OPENER)
        .arg(target)
        .spawn()
        .map(|_child| ())
}
