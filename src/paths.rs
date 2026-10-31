//! Where the settings file lives on each system. Shared with the
//! settings app (which includes this file with `#[path]`).

use std::path::{Path, PathBuf};

/// The settings file's name, inside [`config_dir`].
const CONFIG_FILE: &str = "config.toml";

/// The settings folder, named for the error shown when it can't be
/// found.
#[cfg(windows)]
pub const CONFIG_DIR_NAME: &str = r"%APPDATA%\Wallflex";
/// The settings folder, named for the error shown when it can't be
/// found.
#[cfg(target_os = "macos")]
pub const CONFIG_DIR_NAME: &str = "~/Library/Application Support/Wallflex";
/// The settings folder, named for the error shown when it can't be
/// found.
#[cfg(not(any(windows, target_os = "macos")))]
pub const CONFIG_DIR_NAME: &str = "$XDG_CONFIG_HOME/wallflex";

/// The settings folder: `%APPDATA%\Wallflex` on Windows,
/// `~/Library/Application Support/Wallflex` on macOS, and
/// `$XDG_CONFIG_HOME/wallflex` (default `~/.config/wallflex`)
/// elsewhere. `None` if the variable it's under isn't set.
#[cfg(windows)]
fn config_dir() -> Option<PathBuf> {
    Some(PathBuf::from(std::env::var_os("APPDATA")?).join("Wallflex"))
}

/// The settings folder (see the Windows version).
#[cfg(target_os = "macos")]
fn config_dir() -> Option<PathBuf> {
    let home = PathBuf::from(std::env::var_os("HOME")?);
    Some(home.join("Library/Application Support/Wallflex"))
}

/// The settings folder (see the Windows version).
#[cfg(not(any(windows, target_os = "macos")))]
fn config_dir() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .filter(|dir| !dir.is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config"))
        })?;
    Some(base.join("wallflex"))
}

/// The default settings file, `config.toml` in [`config_dir`].
pub fn default_config_path() -> Option<PathBuf> {
    Some(config_dir()?.join(CONFIG_FILE))
}

/// `path` as this system spells it. The settings app saves library
/// pictures Windows style (`Pictures\sky.jpg`), so where `\` isn't a
/// separator it's read as `/`.
pub fn native_separators(path: &Path) -> PathBuf {
    if cfg!(windows) {
        path.to_path_buf()
    } else {
        PathBuf::from(path.to_string_lossy().replace('\\', "/"))
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::{default_config_path, native_separators};

    #[test]
    fn reads_backslashes_as_separators_off_windows() {
        let path = native_separators(Path::new(r"Pictures\Trips\sky.jpg"));
        let expected = Path::new("Pictures").join("Trips").join("sky.jpg");
        if cfg!(windows) {
            assert_eq!(path.to_str(), Some(r"Pictures\Trips\sky.jpg"));
        } else {
            assert_eq!(path, expected);
        }
    }

    #[test]
    fn ends_in_config_toml() {
        // The variables these come from are set on every test runner.
        let path = default_config_path().unwrap();
        assert!(path.ends_with("config.toml"));
        assert!(path.to_string_lossy().to_lowercase().contains("wallflex"));
    }
}
