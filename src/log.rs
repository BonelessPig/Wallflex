//! A simple log: each line goes to stderr (the console in debug builds)
//! and to `wallflex.log` next to the config, where a friend can find it
//! when something's wrong. The file is started fresh each run.

use std::fs::File;
use std::io::Write as _;
use std::path::Path;
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

/// The open log file, if any, and when logging started.
static LOG: OnceLock<(Mutex<Option<File>>, Instant)> = OnceLock::new();

/// Starts logging to `path` (replacing an old log). Without this, lines
/// only go to stderr.
pub fn open(path: &Path) {
    let file = File::create(path).ok();
    let _ = LOG.set((Mutex::new(file), Instant::now()));
}

/// Writes one line, prefixed with the seconds since start. Use the
/// [`log!`](crate::log!) macro rather than calling this.
pub fn write(message: std::fmt::Arguments<'_>) {
    let elapsed = LOG
        .get()
        .map_or(0.0, |(_, start)| start.elapsed().as_secs_f64());
    let line = format!("[{elapsed:8.1}s] {message}");
    eprintln!("wallflex: {message}");
    if let Some((file, _)) = LOG.get()
        && let Ok(mut file) = file.lock()
        && let Some(file) = file.as_mut()
    {
        // A log that can't be written is no reason to stop.
        let _ = writeln!(file, "{line}");
    }
}

/// Logs a formatted line: `log!("drew {} widgets", n)`.
#[macro_export]
macro_rules! log {
    ($($arg:tt)*) => {
        $crate::log::write(format_args!($($arg)*))
    };
}
