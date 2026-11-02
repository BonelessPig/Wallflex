//! Wallflex guides reader: shows your guides (Markdown, HTML, PDF and
//! text files in the `Guides` folder next to Wallflex's settings) in a
//! borderless window, opened from Wallflex's book icon.
//!
//! It's a separate program so Wallflex itself stays light: the reader
//! uses `WebView2` (Edge's engine, part of Windows 11), which costs real
//! memory, but only while the reader runs. Closing it ends it; it opens
//! again where it was.

// Release builds have no console window; debug builds keep one for logs.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod args;
mod claude;
mod convert;
mod corner;
mod guides;
mod messages;
mod place;
mod saved;
mod style;
mod web;
mod win32;

use std::path::PathBuf;
use std::process::ExitCode;

/// Exit code for a bad command line.
const EXIT_USAGE: u8 = 2;

fn main() -> ExitCode {
    let options = match args::parse(std::env::args().skip(1)) {
        Ok(options) => options,
        Err(error) => {
            eprintln!("wallflex-reader: {error}\n\n{}", args::USAGE);
            return ExitCode::from(EXIT_USAGE);
        }
    };
    if options.help {
        println!("{}", args::USAGE);
        return ExitCode::SUCCESS;
    }
    if options.quit {
        return if win32::quit() {
            ExitCode::SUCCESS
        } else {
            eprintln!("wallflex-reader: no running copy found");
            ExitCode::FAILURE
        };
    }
    let Some(config) = options.config.or_else(default_config_path) else {
        eprintln!("wallflex-reader: can't find %APPDATA%");
        return ExitCode::FAILURE;
    };
    let dir = config.parent().map(PathBuf::from).unwrap_or_default();
    let folder = dir.join(guides::FOLDER);
    if let Err(error) = std::fs::create_dir_all(&folder) {
        // The reader still opens, showing how to add guides.
        eprintln!("wallflex-reader: can't make {}: {error}", folder.display());
    }
    let launch = win32::Launch {
        at: options.at,
        folder,
        saved: dir.join(saved::FILE),
        data: data_dir().unwrap_or_else(|| dir.join("WebView2")),
    };
    match win32::run(&launch) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("wallflex-reader: {error}");
            ExitCode::FAILURE
        }
    }
}

/// `%APPDATA%\Wallflex\config.toml`, as Wallflex uses.
fn default_config_path() -> Option<PathBuf> {
    let app_data = std::env::var_os("APPDATA")?;
    Some(PathBuf::from(app_data).join("Wallflex").join("config.toml"))
}

/// Where `WebView2` keeps the reader's browser data:
/// `%LOCALAPPDATA%\Wallflex\WebView2`.
fn data_dir() -> Option<PathBuf> {
    let local = std::env::var_os("LOCALAPPDATA")?;
    Some(PathBuf::from(local).join("Wallflex").join("WebView2"))
}
