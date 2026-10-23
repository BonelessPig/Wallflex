//! Wallflex: lightweight desktop widgets drawn behind the icons.
//!
//! Milestone 4 draws CPU, network and GPU widgets on a solid background
//! between the wallpaper and the desktop icons, placed on each monitor
//! by the layouts in `config.toml`.

// Release builds have no console window; debug builds keep one for logs.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod args;
mod bmp;
mod canvas;
mod color;
mod config;
mod geometry;
mod log;
mod sensors;
mod text;
mod widgets;
mod win32;

use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

use args::{Command, Options};

/// Exit code for a bad command line.
const EXIT_USAGE: u8 = 2;
/// Gap between the two readings `--sensors` takes (rates need two).
const SENSOR_WARMUP: Duration = Duration::from_secs(1);
/// Readings `--preview` takes, a second apart, so the graph has a line.
const PREVIEW_READINGS: u32 = 4;

fn main() -> ExitCode {
    // Lets `--help`, `--diagnose` and errors print when a release build
    // is started from a terminal.
    win32::console::attach_parent();

    let options = match args::parse(std::env::args().skip(1)) {
        Ok(options) => options,
        Err(error) => {
            eprintln!("wallflex: {error}\n\n{}", args::USAGE);
            return ExitCode::from(EXIT_USAGE);
        }
    };

    match options.command.clone() {
        Command::Help => {
            println!("{}", args::USAGE);
            ExitCode::SUCCESS
        }
        Command::Sensors => {
            let mut sensors = win32::sensors::Sensors::new();
            let _first = sensors.sample();
            std::thread::sleep(SENSOR_WARMUP);
            for line in text::lines(&sensors.sample()) {
                println!("{line}");
            }
            ExitCode::SUCCESS
        }
        Command::Preview { height, path } => {
            preview(height, &path, options.background)
        }
        Command::Diagnose => {
            println!("{}", win32::app::diagnose());
            ExitCode::SUCCESS
        }
        Command::Quit => {
            if win32::app::request_quit() {
                ExitCode::SUCCESS
            } else {
                eprintln!("wallflex: no running copy found");
                ExitCode::FAILURE
            }
        }
        Command::Run => start(options),
    }
}

/// Runs Wallflex: finds the settings file (writing the commented default
/// on first run), starts the log next to it, then draws until quit.
fn start(options: Options) -> ExitCode {
    let custom = options.config.as_ref().map(PathBuf::from);
    let config_path = custom.clone().or_else(default_config_path);
    if let Some(path) = &config_path {
        if let Some(dir) = path.parent() {
            // If this fails, the log and default file just aren't written.
            let _ = std::fs::create_dir_all(dir);
            log::open(&dir.join("wallflex.log"));
        }
        if custom.is_none() && !path.exists() {
            match std::fs::write(path, config::DEFAULT_CONFIG) {
                Ok(()) => {
                    crate::log!("wrote default settings to {}", path.display());
                }
                Err(error) => {
                    crate::log!("can't write {}: {error}", path.display());
                }
            }
        }
    }
    match win32::app::run(options, config_path) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            crate::log!("{error}");
            ExitCode::FAILURE
        }
    }
}

/// `%APPDATA%\Wallflex\config.toml`.
fn default_config_path() -> Option<PathBuf> {
    let app_data = std::env::var_os("APPDATA")?;
    Some(PathBuf::from(app_data).join("Wallflex").join("config.toml"))
}

/// Renders the widgets with live readings, as they'd look on a monitor
/// `height` pixels tall, and saves them as a BMP at `path`.
fn preview(
    height: u32,
    path: &str,
    background: Option<color::Rgb>,
) -> ExitCode {
    let mut sensors = win32::sensors::Sensors::new();
    let mut dashboard = widgets::Dashboard::default();
    for _ in 0..PREVIEW_READINGS {
        dashboard.update(sensors.sample());
        std::thread::sleep(SENSOR_WARMUP);
    }
    let background = background.unwrap_or(color::Rgb::BLACK);
    let image = win32::paint::preview(
        &dashboard,
        widgets::Widget::Panel,
        height,
        background,
    )
    .and_then(|(w, h, pixels)| bmp::encode(w, h, &pixels));
    let Some(image) = image else {
        eprintln!("wallflex: couldn't render the preview");
        return ExitCode::FAILURE;
    };
    match std::fs::write(path, image) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("wallflex: can't write {path}: {error}");
            ExitCode::FAILURE
        }
    }
}
