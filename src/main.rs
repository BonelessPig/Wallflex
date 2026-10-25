//! Wallflex: lightweight desktop widgets drawn behind the icons.
//!
//! Milestone 4 draws CPU, network and GPU widgets on a solid background
//! between the wallpaper and the desktop icons, placed on each monitor
//! by the layouts in `config.toml`.

// Release builds have no console window; debug builds keep one for logs.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod args;
mod audio;
mod backdrop;
mod bmp;
mod canvas;
mod color;
mod config;
mod discord;
mod geometry;
#[cfg(test)]
mod icon;
mod image;
mod log;
mod media;
mod pace;
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
        Command::Preview { height, path } => preview(height, &path, &options),
        Command::Check => check(options.config.as_deref()),
        Command::Info => {
            println!("{}", info());
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

/// Reads and checks a settings file: `--config`'s, or the default one.
///
/// # Errors
///
/// What's wrong, ready to show the user.
fn load_settings(path: Option<&str>) -> Result<config::Settings, String> {
    let path = path
        .map(PathBuf::from)
        .or_else(default_config_path)
        .ok_or("can't find the settings folder (%APPDATA%)")?;
    let text = std::fs::read_to_string(&path)
        .map_err(|e| format!("can't read {}: {e}", path.display()))?;
    let settings = config::parse(&text).map_err(|e| e.to_string())?;
    let folder = path.parent().unwrap_or_else(|| std::path::Path::new(""));
    Ok(settings.with_base_dir(folder))
}

/// `--check`: says whether the settings file is valid, and if not why.
fn check(path: Option<&str>) -> ExitCode {
    match load_settings(path) {
        Ok(_) => {
            println!("ok");
            ExitCode::SUCCESS
        }
        Err(error) => {
            println!("{error}");
            ExitCode::FAILURE
        }
    }
}

/// `--info`: the monitors and GPUs, as JSON for the settings app.
fn info() -> serde_json::Value {
    let rect = |r: geometry::Rect| {
        serde_json::json!({
            "left": r.left, "top": r.top, "right": r.right, "bottom": r.bottom,
        })
    };
    let monitors: Vec<_> = win32::monitors::list()
        .into_iter()
        .map(|m| {
            serde_json::json!({
                "name": m.name.trim_start_matches(r"\\.\"),
                "rect": rect(m.rect),
                "work": rect(m.work),
                "primary": m.primary,
            })
        })
        .collect();
    let gpus: Vec<_> = win32::sensors::Sensors::new()
        .sample()
        .gpus
        .into_iter()
        .map(|g| g.name)
        .collect();
    serde_json::json!({ "monitors": monitors, "gpus": gpus })
}

/// Renders the widgets with live readings, as they'd look on a monitor
/// `height` pixels tall, and saves them as a BMP at `path` (or, for
/// `--widget all`, one BMP per widget in the folder `path`). With
/// `--config`, that file's panel parts, graph length and background
/// apply.
fn preview(height: u32, path: &str, options: &Options) -> ExitCode {
    use args::PreviewWidget;
    use widgets::Widget;
    let settings =
        match options.config.as_deref().map(|p| load_settings(Some(p))) {
            Some(Ok(settings)) => Some(settings),
            Some(Err(error)) => {
                eprintln!("wallflex: {error}");
                return ExitCode::FAILURE;
            }
            None => None,
        };
    let mut sensors = win32::sensors::Sensors::new();
    let mut dashboard = widgets::Dashboard::default();
    if let Some(settings) = &settings {
        dashboard.set_parts(settings.parts.clone());
        dashboard.set_net_seconds(settings.net_seconds);
    }
    for _ in 0..PREVIEW_READINGS {
        dashboard.update(sensors.sample());
        std::thread::sleep(SENSOR_WARMUP);
    }
    dashboard.settle();
    // A transparent background previews on black, as it'd look over a
    // dark wallpaper.
    let background = options
        .background
        .or(settings.and_then(|s| match s.background {
            backdrop::Background::Color(color) => Some(color),
            backdrop::Background::Transparent => None,
        }))
        .unwrap_or(color::Rgb::BLACK);
    let kind = options.widget;
    let widget = match kind {
        PreviewWidget::Panel => Widget::Panel,
        PreviewWidget::Cpu => Widget::Cpu,
        PreviewWidget::Network => Widget::Network,
        PreviewWidget::Gpus => Widget::Gpus,
        PreviewWidget::Info => Widget::Info,
        PreviewWidget::Visualizer => {
            dashboard.show_visualizer_demo();
            Widget::Visualizer
        }
        PreviewWidget::Discord
        | PreviewWidget::DiscordAvatars
        | PreviewWidget::DiscordInbox => {
            dashboard.show_discord_demo(match kind {
                PreviewWidget::DiscordAvatars => widgets::Demo::Avatars,
                PreviewWidget::DiscordInbox => widgets::Demo::Inbox,
                _ => widgets::Demo::Names,
            });
            fetch_preview_images(&mut dashboard);
            Widget::Discord
        }
        PreviewWidget::All => {
            return preview_all(&mut dashboard, height, path, background);
        }
    };
    if save_preview(&dashboard, widget, height, background, path) {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

/// Most single GPUs `--widget all` saves (`gpu1.bmp` and up).
const PREVIEW_MAX_GPUS: usize = 8;

/// `--widget all`: saves every widget into the folder `dir`, as
/// `panel.bmp`, `cpu.bmp`, ..., `gpu1.bmp` for each GPU, with the
/// visualizer and Discord showing sample data (no avatars, so it's
/// quick). Widgets with nothing to show are left out.
fn preview_all(
    dashboard: &mut widgets::Dashboard,
    height: u32,
    dir: &str,
    background: color::Rgb,
) -> ExitCode {
    use widgets::Widget;
    if let Err(error) = std::fs::create_dir_all(dir) {
        eprintln!("wallflex: can't make {dir}: {error}");
        return ExitCode::FAILURE;
    }
    dashboard.show_visualizer_demo();
    dashboard.show_discord_demo(widgets::Demo::Names);
    let mut all = vec![
        ("panel".to_owned(), Widget::Panel),
        ("cpu".to_owned(), Widget::Cpu),
        ("network".to_owned(), Widget::Network),
        ("gpus".to_owned(), Widget::Gpus),
        ("info".to_owned(), Widget::Info),
        ("visualizer".to_owned(), Widget::Visualizer),
        ("discord".to_owned(), Widget::Discord),
    ];
    for (number, index) in (1_u32..).zip(0..PREVIEW_MAX_GPUS) {
        all.push((format!("gpu{number}"), Widget::Gpu(index)));
    }
    let mut ok = true;
    for (name, widget) in all {
        let (width, height_units) = dashboard.size(widget);
        if width <= 0.0 || height_units <= 0.0 {
            continue;
        }
        let path = std::path::Path::new(dir).join(format!("{name}.bmp"));
        let path = path.to_string_lossy();
        ok &= save_preview(dashboard, widget, height, background, &path);
    }
    if ok {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

/// Renders one widget and saves it as a BMP at `path`. Says what went
/// wrong on stderr and returns false if it couldn't.
fn save_preview(
    dashboard: &widgets::Dashboard,
    widget: widgets::Widget,
    height: u32,
    background: color::Rgb,
    path: &str,
) -> bool {
    let image = win32::paint::preview(dashboard, widget, height, background)
        .and_then(|(w, h, pixels)| bmp::encode(w, h, &pixels));
    let Some(image) = image else {
        eprintln!("wallflex: couldn't render the preview");
        return false;
    };
    match std::fs::write(path, image) {
        Ok(()) => true,
        Err(error) => {
            eprintln!("wallflex: can't write {path}: {error}");
            false
        }
    }
}

/// Longest wait for `--preview`'s pictures.
const PREVIEW_IMAGE_WAIT: Duration = Duration::from_secs(10);
/// How often `--preview` checks for them.
const PREVIEW_IMAGE_POLL: Duration = Duration::from_millis(100);

/// Downloads the pictures the preview's sample data uses, waiting up to
/// [`PREVIEW_IMAGE_WAIT`].
fn fetch_preview_images(dashboard: &mut widgets::Dashboard) {
    let wanted = dashboard.wanted_images();
    // No window to notify: the preview checks for itself.
    let mut fetcher = win32::images::ImageFetcher::start(
        windows::Win32::Foundation::HWND::default(),
        0,
    );
    for url in &wanted {
        fetcher.fetch(url);
    }
    let start = std::time::Instant::now();
    let mut received = 0;
    while received < wanted.len() && start.elapsed() < PREVIEW_IMAGE_WAIT {
        std::thread::sleep(PREVIEW_IMAGE_POLL);
        for (url, image) in fetcher.take_ready() {
            dashboard.add_image(url, image);
            received = received.saturating_add(1);
        }
    }
    crate::log!("preview: {received} of {} pictures", wanted.len());
}
