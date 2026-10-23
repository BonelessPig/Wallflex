//! Wallflex: lightweight desktop widgets drawn behind the icons.
//!
//! Milestone 2 draws a solid background on every monitor, between the
//! wallpaper and the desktop icons, with a plain-text sensor panel.

// Release builds have no console window; debug builds keep one for logs.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod args;
mod color;
mod geometry;
mod sensors;
mod text;
mod win32;

use std::process::ExitCode;
use std::time::Duration;

use args::Command;

/// Exit code for a bad command line.
const EXIT_USAGE: u8 = 2;
/// Gap between the two readings `--sensors` takes (rates need two).
const SENSOR_WARMUP: Duration = Duration::from_secs(1);

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

    match options.command {
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
