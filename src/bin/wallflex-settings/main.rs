//! Wallflex Settings: an optional app for editing `config.toml` without
//! writing TOML by hand.
//!
//! It serves one page on `127.0.0.1` and opens it in an app window
//! (Microsoft Edge's `--app` mode, or the default browser). The page
//! shows the monitors with the widgets where they'd go, so they can be
//! dragged into place, and has forms for everything else in the file.
//! Checking, previews and monitor details come from `wallflex.exe`
//! itself (`--check`, `--preview ... --widget all`, `--info`), so this
//! app never disagrees with it. Saving writes the file; the running
//! Wallflex picks it up within a second, as it does any edit.
//!
//! It exits a few seconds after its window closes.

// Release builds have no console window; debug builds keep one for logs.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod http;
mod library;
#[path = "../../paths.rs"]
mod paths;
mod server;
mod window;

use std::path::PathBuf;
use std::process::ExitCode;

/// Help text for `--help` and usage errors.
const USAGE: &str = "\
Usage: wallflex-settings [options]

Opens a window for editing Wallflex's settings (config.toml in
%APPDATA%\\Wallflex on Windows, ~/Library/Application Support/Wallflex
on macOS, ~/.config/wallflex elsewhere).

Options:
  --config <FILE>  edit this settings file instead
  --browser        open in the default browser, not an app window
  --no-open        don't open a window; just print the address
  -h, --help       show this help";

/// Exit code for a bad command line.
const EXIT_USAGE: u8 = 2;

/// Command-line options.
#[derive(Debug, Default, PartialEq, Eq)]
struct Options {
    /// Settings file from `--config`.
    config: Option<PathBuf>,
    /// Use the default browser.
    browser: bool,
    /// Don't open anything.
    no_open: bool,
    /// Show the help.
    help: bool,
}

/// Parses the arguments after the program name.
fn parse_args<I: IntoIterator<Item = String>>(
    args: I,
) -> Result<Options, String> {
    let mut options = Options::default();
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--config" => {
                let file = args.next().ok_or("--config needs a file")?;
                options.config = Some(PathBuf::from(file));
            }
            "--browser" => options.browser = true,
            "--no-open" => options.no_open = true,
            "-h" | "--help" => options.help = true,
            other => return Err(format!("unknown argument '{other}'")),
        }
    }
    Ok(options)
}

fn main() -> ExitCode {
    let options = match parse_args(std::env::args().skip(1)) {
        Ok(options) => options,
        Err(error) => {
            eprintln!("wallflex-settings: {error}\n\n{USAGE}");
            return ExitCode::from(EXIT_USAGE);
        }
    };
    if options.help {
        println!("{USAGE}");
        return ExitCode::SUCCESS;
    }
    let Some(config) =
        options.config.clone().or_else(paths::default_config_path)
    else {
        eprintln!("wallflex-settings: can't find {}", paths::CONFIG_DIR_NAME);
        return ExitCode::FAILURE;
    };
    let exe_name = format!("wallflex{}", std::env::consts::EXE_SUFFIX);
    let wallflex = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|dir| dir.join(&exe_name)))
        .unwrap_or_else(|| PathBuf::from(&exe_name));
    let server = match server::Server::start(config, wallflex) {
        Ok(server) => server,
        Err(error) => {
            eprintln!("wallflex-settings: can't start: {error}");
            return ExitCode::FAILURE;
        }
    };
    let url = server.url();
    println!("Wallflex Settings: {url}");
    if !options.no_open {
        window::open(&url, options.browser);
    }
    server.run();
    ExitCode::SUCCESS
}

#[cfg(test)]
mod tests {
    use super::{Options, parse_args};
    use std::path::PathBuf;

    fn run(args: &[&str]) -> Result<Options, String> {
        parse_args(args.iter().map(|a| (*a).to_owned()))
    }

    #[test]
    fn reads_options() {
        assert_eq!(run(&[]), Ok(Options::default()));
        let options = run(&["--config", "a.toml", "--browser"]).unwrap();
        assert_eq!(options.config, Some(PathBuf::from("a.toml")));
        assert!(options.browser && !options.no_open);
        assert!(run(&["--config"]).is_err());
        assert!(run(&["--bogus"]).is_err());
    }
}
