//! Opens the settings page: in an Edge app window (no tabs or address
//! bar, so it feels like an app) or, failing that, the default browser.

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
    // Explorer hands a URL to the default browser.
    if let Err(error) = Command::new("explorer.exe").arg(url).spawn() {
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
