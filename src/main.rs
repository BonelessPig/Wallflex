//! Wallflex: lightweight desktop widgets drawn behind the icons.
//!
//! Milestone 3 draws a solid background on every monitor, between the
//! wallpaper and the desktop icons, with native CPU, network and GPU
//! widgets.

// Release builds have no console window; debug builds keep one for logs.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod args;
mod bmp;
mod canvas;
mod color;
mod geometry;
mod sensors;
mod text;
mod widgets;
mod win32;

use std::process::ExitCode;
use std::time::Duration;

use args::Command;

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
        Command::Run => match win32::app::run(options) {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("wallflex: {error}");
                ExitCode::FAILURE
            }
        },
    }
}

/// Renders the widgets with live readings, as they'd look on a monitor
/// `height` pixels tall, and saves them as a BMP at `path`.
fn preview(height: u32, path: &str, background: color::Rgb) -> ExitCode {
    let mut sensors = win32::sensors::Sensors::new();
    let mut dashboard = widgets::Dashboard::default();
    for _ in 0..PREVIEW_READINGS {
        dashboard.update(sensors.sample());
        std::thread::sleep(SENSOR_WARMUP);
    }
    let image = win32::paint::Painter::new(background)
        .ok()
        .and_then(|mut painter| painter.preview(&dashboard, height))
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
