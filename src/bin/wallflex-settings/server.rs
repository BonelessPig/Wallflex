//! The local server behind the settings page.
//!
//! It listens on `127.0.0.1` only, on a port the system picks. Every API
//! call must carry the random token from the page's address, and the
//! `Host` header must name this server, so other web pages (or DNS
//! tricks) can't read or change the settings through it.

use std::ffi::OsStr;
use std::net::{TcpListener, TcpStream};
#[cfg(windows)]
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant, SystemTime};

use serde_json::{Value, json};

use crate::http::{
    MAX_BODY_BYTES, Request, Response, percent_decode, read_request,
};
use crate::library::{self, picture_type};
use crate::window;

/// The page.
const PAGE: &str = include_str!("ui.html");
/// The commented default settings, shown when there's no file yet.
const DEFAULT_CONFIG: &str = include_str!("../../../config.default.toml");
/// `CREATE_NO_WINDOW`: runs `wallflex.exe` without a console flashing.
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;
/// The monitor height previews are drawn for: 1 pixel per design unit.
const PREVIEW_HEIGHT: &str = "1000";
/// How long a connection may take to send its request.
const READ_TIMEOUT: Duration = Duration::from_secs(10);
/// How often the watchdog looks at the clock.
const WATCH_EVERY: Duration = Duration::from_secs(1);
/// After the page says goodbye, how long to wait for it to come back
/// (a reload says goodbye too).
const BYE_GRACE: Duration = Duration::from_secs(4);
/// Quit when nothing has been heard for this long: the page pings
/// every few seconds, or once a minute when its window is hidden.
const IDLE_LIMIT: Duration = Duration::from_secs(5 * 60);
/// Quit if the page hasn't loaded this long after starting.
const START_LIMIT: Duration = Duration::from_secs(2 * 60);
/// The token header the page sends.
const TOKEN_HEADER: &str = "x-wallflex-token";
/// The header carrying the file's stamp when the page loaded it.
const STAMP_HEADER: &str = "x-wallflex-stamp";
/// What the page's address starts with for preview pictures.
const PREVIEW_PREFIX: &str = "/api/preview/";
/// Preview picture extension.
const PREVIEW_EXT: &str = ".bmp";
/// Where the page sends a picture to import; the only address that
/// takes a body bigger than [`MAX_BODY_BYTES`].
const IMPORT_PATH: &str = "/api/library/import";
/// Where the page loads library pictures from (`<img>` can't send the
/// token header, so it's in the address).
const LIBRARY_FILE_PATH: &str = "/api/library/file";
/// The header naming an imported picture (percent-encoded), relative to
/// the library.
const NAME_HEADER: &str = "x-wallflex-name";

/// The running server.
pub struct Server {
    /// The socket.
    listener: TcpListener,
    /// Everything requests need.
    shared: Arc<Shared>,
}

/// What every request handler needs.
struct Shared {
    /// The port, for checking `Host`.
    port: u16,
    /// The page's secret.
    token: String,
    /// The settings file.
    config: PathBuf,
    /// `wallflex.exe`.
    wallflex: PathBuf,
    /// A private folder for drafts and previews, removed on exit.
    temp: PathBuf,
    /// When the page was last heard from.
    last_seen: Mutex<Instant>,
    /// Whether the page has loaded at all.
    seen: AtomicBool,
    /// Whether the page said goodbye.
    bye: AtomicBool,
    /// Numbers draft files, so requests don't share one.
    drafts: AtomicU32,
    /// Held while a preview is drawn: one at a time.
    preview: Mutex<()>,
    /// `wallflex --info`, once read.
    info: Mutex<Option<Value>>,
}

impl Server {
    /// Starts listening.
    ///
    /// # Errors
    ///
    /// The socket's error, if it can't listen.
    pub fn start(config: PathBuf, wallflex: PathBuf) -> std::io::Result<Self> {
        let listener = TcpListener::bind(("127.0.0.1", 0))?;
        let port = listener.local_addr()?.port();
        let temp = std::env::temp_dir()
            .join(format!("wallflex-settings-{}", std::process::id()));
        std::fs::create_dir_all(&temp)?;
        let shared = Arc::new(Shared {
            port,
            token: new_token(),
            config,
            wallflex,
            temp,
            last_seen: Mutex::new(Instant::now()),
            seen: AtomicBool::new(false),
            bye: AtomicBool::new(false),
            drafts: AtomicU32::new(0),
            preview: Mutex::new(()),
            info: Mutex::new(None),
        });
        Ok(Self { listener, shared })
    }

    /// The page's address, token included.
    pub fn url(&self) -> String {
        format!(
            "http://127.0.0.1:{}/?t={}",
            self.shared.port, self.shared.token
        )
    }

    /// Serves until the page goes away, then exits the process.
    pub fn run(self) {
        let watched = Arc::clone(&self.shared);
        std::thread::spawn(move || watchdog(&watched));
        for stream in self.listener.incoming() {
            let Ok(stream) = stream else { continue };
            let shared = Arc::clone(&self.shared);
            std::thread::spawn(move || serve(&shared, stream));
        }
    }
}

/// A hard-to-guess token: two hashes from Rust's randomly keyed hasher,
/// mixed with the time.
fn new_token() -> String {
    use std::fmt::Write as _;
    use std::hash::{BuildHasher, Hasher};
    let nanos = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    let mut token = String::new();
    for round in 0_u8..2 {
        let mut hasher =
            std::collections::hash_map::RandomState::new().build_hasher();
        hasher.write_u8(round);
        hasher.write_u128(nanos);
        // Writing to a String can't fail.
        let _ = write!(token, "{:016x}", hasher.finish());
    }
    token
}

/// Exits once the page has gone: soon after it says goodbye, or after a
/// long silence.
fn watchdog(shared: &Shared) {
    loop {
        std::thread::sleep(WATCH_EVERY);
        let quiet = lock(&shared.last_seen).elapsed();
        let gone = if shared.seen.load(Ordering::Relaxed) {
            (shared.bye.load(Ordering::Relaxed) && quiet > BYE_GRACE)
                || quiet > IDLE_LIMIT
        } else {
            quiet > START_LIMIT
        };
        if gone {
            // Best effort: a leftover temp folder does no harm.
            let _ = std::fs::remove_dir_all(&shared.temp);
            std::process::exit(0);
        }
    }
}

/// Locks a mutex, carrying on if another thread panicked with it held
/// (the data is a timestamp or a cache, still usable).
fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Answers one connection.
fn serve(shared: &Shared, mut stream: TcpStream) {
    // Without the timeout a stalled connection just keeps its thread.
    let _ = stream.set_read_timeout(Some(READ_TIMEOUT));
    let response = match read_request(&mut stream, |r| body_limit(shared, r)) {
        Ok(request) => route(shared, &request),
        Err(error) => Response::text(400, &error.to_string()),
    };
    // The page retries or reports it; nothing to do here.
    let _ = response.write_to(&mut stream);
}

/// The biggest body `request` may have: a picture for an import that
/// carries the token, else [`MAX_BODY_BYTES`], so nobody without the
/// token can make this app hold a big body.
fn body_limit(shared: &Shared, request: &Request) -> usize {
    let import = request.path == IMPORT_PATH
        && request.header(TOKEN_HEADER) == Some(shared.token.as_str());
    if import {
        library::MAX_BYTES
    } else {
        MAX_BODY_BYTES
    }
}

/// Picks the handler for a request, after the security checks.
fn route(shared: &Shared, request: &Request) -> Response {
    let host = format!("127.0.0.1:{}", shared.port);
    if request.header("host") != Some(host.as_str()) {
        return Response::text(403, "wrong host");
    }
    let method = request.method.as_str();
    let path = request.path.as_str();
    if (method, path) == ("GET", "/") {
        return Response {
            status: 200,
            content_type: "text/html; charset=utf-8",
            body: PAGE.as_bytes().to_vec(),
        };
    }
    // The goodbye comes from `navigator.sendBeacon` and library pictures
    // from `<img>`, which can't add headers, so their token is in the
    // address.
    let token = if path == "/api/bye" || path == LIBRARY_FILE_PATH {
        request.param("t")
    } else {
        request.header(TOKEN_HEADER)
    };
    if token != Some(shared.token.as_str()) {
        return Response::text(403, "missing token");
    }
    shared.heard_from();
    let body = String::from_utf8_lossy(&request.body);
    match (method, path) {
        ("GET", "/api/state") => Response::json(200, &shared.state()),
        ("GET", "/api/info") => Response::json(200, &shared.info()),
        ("POST", "/api/check") => Response::json(200, &shared.check(&body)),
        ("POST", "/api/save") => {
            shared.save(&body, request.header(STAMP_HEADER))
        }
        ("POST", "/api/preview") => Response::json(200, &shared.preview(&body)),
        ("POST", "/api/ping") => Response::json(200, &json!({})),
        ("POST", "/api/picture") => shared.picture(body.trim()),
        ("GET", "/api/library") => Response::json(200, &shared.library()),
        ("GET", LIBRARY_FILE_PATH) => {
            let name = request.param("n").and_then(percent_decode);
            shared.library_file(name.as_deref().unwrap_or_default())
        }
        ("POST", IMPORT_PATH) => {
            let name = request.header(NAME_HEADER).and_then(percent_decode);
            let name = name.unwrap_or_default();
            Response::json(200, &shared.import(&name, &request.body))
        }
        ("POST", "/api/library/delete") => {
            Response::json(200, &shared.delete(body.trim()))
        }
        ("POST", "/api/library/open") => {
            Response::json(200, &shared.open_library())
        }
        ("POST", "/api/bye") => {
            shared.bye.store(true, Ordering::Relaxed);
            Response::json(200, &json!({}))
        }
        ("GET", _) => match preview_name(path) {
            Some(name) => shared.preview_image(name),
            None => Response::text(404, "not found"),
        },
        _ => Response::text(404, "not found"),
    }
}

/// The widget name in a preview picture's address
/// (`/api/preview/panel.bmp` gives `panel`), if it's a safe file name.
fn preview_name(path: &str) -> Option<&str> {
    let name = path
        .strip_prefix(PREVIEW_PREFIX)?
        .strip_suffix(PREVIEW_EXT)?;
    let safe = !name.is_empty()
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit());
    safe.then_some(name)
}

/// What `wallflex.exe` printed and whether it succeeded.
struct Ran {
    /// Exit code 0.
    ok: bool,
    /// Standard output, trimmed.
    stdout: String,
    /// Standard error, trimmed.
    stderr: String,
}

impl Shared {
    /// Notes that the page is still there.
    fn heard_from(&self) {
        *lock(&self.last_seen) = Instant::now();
        self.seen.store(true, Ordering::Relaxed);
        self.bye.store(false, Ordering::Relaxed);
    }

    /// Runs `wallflex.exe` with `args` and waits for it.
    fn run(&self, args: &[&OsStr]) -> Result<Ran, String> {
        let mut command = Command::new(&self.wallflex);
        command.args(args);
        #[cfg(windows)]
        command.creation_flags(CREATE_NO_WINDOW);
        let output = command.output().map_err(|e| {
            format!("can't run {}: {e}", self.wallflex.display())
        })?;
        Ok(Ran {
            ok: output.status.success(),
            stdout: String::from_utf8_lossy(&output.stdout).trim().to_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).trim().to_owned(),
        })
    }

    /// Writes `text` to a new draft file and returns its path.
    fn draft(&self, text: &str) -> Result<PathBuf, String> {
        let number = self.drafts.fetch_add(1, Ordering::Relaxed);
        let path = self.temp.join(format!("draft-{number}.toml"));
        std::fs::write(&path, text)
            .map_err(|e| format!("can't write {}: {e}", path.display()))?;
        Ok(path)
    }

    /// Asks `wallflex --check` about `text`: `Ok` if it's valid, else
    /// what's wrong.
    fn validate(&self, text: &str) -> Result<(), String> {
        let draft = self.draft(text)?;
        let ran = self.run(&[
            OsStr::new("--check"),
            OsStr::new("--config"),
            draft.as_os_str(),
        ]);
        // A leftover draft is cleaned up with the folder on exit.
        let _ = std::fs::remove_file(&draft);
        let ran = ran?;
        if ran.ok {
            Ok(())
        } else if ran.stdout.is_empty() {
            Err(format!("wallflex --check failed: {}", ran.stderr))
        } else {
            Err(ran.stdout)
        }
    }

    /// The file as it is now: its text, the same as JSON (if it's valid
    /// TOML), whether Wallflex accepts it, and its stamp.
    fn state(&self) -> Value {
        let exists = self.config.is_file();
        let text = if exists {
            match std::fs::read_to_string(&self.config) {
                Ok(text) => text,
                Err(error) => {
                    return json!({ "fatal": format!(
                        "can't read {}: {error}", self.config.display()
                    )});
                }
            }
        } else {
            DEFAULT_CONFIG.to_owned()
        };
        let parsed = toml::from_str::<Value>(&text);
        let problem = match &parsed {
            Ok(_) => self.validate(&text).err(),
            Err(error) => Some(error.to_string()),
        };
        json!({
            "path": self.config.display().to_string(),
            "exists": exists,
            "config": parsed.ok(),
            "problem": problem,
            "stamp": stamp(&self.config),
            "defaults": toml::from_str::<Value>(DEFAULT_CONFIG).ok(),
        })
    }

    /// The monitors and GPUs, from `wallflex --info` (read once).
    fn info(&self) -> Value {
        let mut cached = lock(&self.info);
        if let Some(info) = cached.as_ref() {
            return info.clone();
        }
        let info = match self.run(&[OsStr::new("--info")]) {
            Ok(ran) if ran.ok => serde_json::from_str(&ran.stdout)
                .unwrap_or_else(|e| json!({ "error": e.to_string() })),
            Ok(ran) => json!({ "error": ran.stderr }),
            Err(error) => json!({ "error": error }),
        };
        if info.get("error").is_none() {
            *cached = Some(info.clone());
        }
        info
    }

    /// Checks `text` without saving it.
    fn check(&self, text: &str) -> Value {
        match self.validate(text) {
            Ok(()) => json!({ "ok": true }),
            Err(error) => json!({ "ok": false, "error": error }),
        }
    }

    /// Saves `text` if Wallflex accepts it and the file hasn't changed
    /// since the page read it (`loaded` is the stamp it had then). The
    /// old file is kept as `config.toml.bak`.
    fn save(&self, text: &str, loaded: Option<&str>) -> Response {
        if let Some(loaded) = loaded
            && loaded != stamp(&self.config)
        {
            return Response::json(
                409,
                &json!({
                    "ok": false,
                    "error": "The settings file was changed somewhere else \
                              since this window loaded it.",
                }),
            );
        }
        if let Err(error) = self.validate(text) {
            return Response::json(
                200,
                &json!({ "ok": false, "error": error }),
            );
        }
        match write_config(&self.config, text) {
            Ok(()) => Response::json(
                200,
                &json!({
                    "ok": true,
                    "stamp": stamp(&self.config),
                }),
            ),
            Err(error) => {
                Response::json(200, &json!({ "ok": false, "error": error }))
            }
        }
    }

    /// Draws every widget for `text`'s settings with live readings, into
    /// the preview folder. Returns the widget names drawn.
    fn preview(&self, text: &str) -> Value {
        let _one_at_a_time = lock(&self.preview);
        let folder = self.temp.join("preview");
        // It may not exist yet; a failure shows when drawing.
        let _ = std::fs::remove_dir_all(&folder);
        let result = self.draft(text).and_then(|draft| {
            let ran = self.run(&[
                OsStr::new("--config"),
                draft.as_os_str(),
                OsStr::new("--preview"),
                OsStr::new(PREVIEW_HEIGHT),
                folder.as_os_str(),
                OsStr::new("--widget"),
                OsStr::new("all"),
            ]);
            let _ = std::fs::remove_file(&draft);
            ran
        });
        match result {
            Ok(ran) if ran.ok => {
                json!({ "ok": true, "images": names(&folder) })
            }
            Ok(ran) => json!({ "ok": false, "error": ran.stderr }),
            Err(error) => json!({ "ok": false, "error": error }),
        }
    }

    /// The picture file at `path` (relative paths are next to the
    /// settings file, as wallflex reads them), for the page to show on
    /// its monitors.
    fn picture(&self, path: &str) -> Response {
        let path = crate::paths::native_separators(Path::new(path));
        let full = if path.is_relative() {
            self.config_dir().join(path)
        } else {
            path
        };
        send_picture(&full)
    }

    /// The settings file's folder.
    fn config_dir(&self) -> &Path {
        self.config.parent().unwrap_or(Path::new(""))
    }

    /// The picture library's folder.
    fn library_dir(&self) -> PathBuf {
        self.config_dir().join(library::FOLDER)
    }

    /// The library's folder and pictures. Each picture has its `name`
    /// in the library and the `path` settings use for it.
    fn library(&self) -> Value {
        let pictures: Vec<Value> = library::list(&self.library_dir())
            .into_iter()
            .map(|entry| {
                json!({
                    "path": settings_path(&entry.name),
                    "name": entry.name,
                    "bytes": entry.bytes,
                })
            })
            .collect();
        json!({
            "folder": self.library_dir().display().to_string(),
            "pictures": pictures,
        })
    }

    /// A library picture, by its name in the library.
    fn library_file(&self, name: &str) -> Response {
        match library::resolve(&self.library_dir(), name) {
            Some(path) => send_picture(&path),
            None => Response::text(400, "not a library picture"),
        }
    }

    /// Copies an uploaded picture into the library as `name`.
    fn import(&self, name: &str, bytes: &[u8]) -> Value {
        match library::import(&self.library_dir(), name, bytes) {
            Ok(done) => json!({
                "ok": true,
                "path": settings_path(&done.name),
                "name": done.name,
                "existed": done.existed,
            }),
            Err(error) => json!({ "ok": false, "error": error }),
        }
    }

    /// Deletes the library picture `name`.
    fn delete(&self, name: &str) -> Value {
        match library::delete(&self.library_dir(), name) {
            Ok(()) => json!({ "ok": true }),
            Err(error) => json!({ "ok": false, "error": error }),
        }
    }

    /// Opens the library in Explorer (or Finder, or the file manager),
    /// making it first if need be.
    fn open_library(&self) -> Value {
        let folder = self.library_dir();
        let opened = std::fs::create_dir_all(&folder)
            .and_then(|()| window::open_with_system(folder.as_os_str()));
        match opened {
            // The file manager runs on its own; nothing to wait for.
            Ok(()) => json!({ "ok": true }),
            Err(error) => json!({
                "ok": false,
                "error": format!("can't open {}: {error}", folder.display()),
            }),
        }
    }

    /// One preview picture.
    fn preview_image(&self, name: &str) -> Response {
        let file = self
            .temp
            .join("preview")
            .join(format!("{name}{PREVIEW_EXT}"));
        match std::fs::read(file) {
            Ok(body) => Response {
                status: 200,
                content_type: "image/bmp",
                body,
            },
            Err(_) => Response::text(404, "no such preview"),
        }
    }
}

/// The path settings use for the library picture `name`: relative to
/// the settings file's folder, where Wallflex looks for it.
fn settings_path(name: &str) -> String {
    format!("{}\\{name}", library::FOLDER)
}

/// The picture file at `path`, if it is one and isn't too big.
fn send_picture(path: &Path) -> Response {
    let Some(content_type) = picture_type(path) else {
        return Response::text(400, "not a picture file");
    };
    let limit = u64::try_from(library::MAX_BYTES).unwrap_or(u64::MAX);
    let size = std::fs::metadata(path).map(|m| m.len());
    if size.is_ok_and(|bytes| bytes > limit) {
        return Response::text(400, "picture too big to preview");
    }
    match std::fs::read(path) {
        Ok(body) => Response {
            status: 200,
            content_type,
            body,
        },
        Err(error) => Response::text(404, &error.to_string()),
    }
}

/// The widget names of the pictures in a preview folder.
fn names(folder: &Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(folder) else {
        return Vec::new();
    };
    let mut names: Vec<String> = entries
        .filter_map(Result::ok)
        .filter_map(|e| e.file_name().into_string().ok())
        .filter_map(|n| n.strip_suffix(PREVIEW_EXT).map(str::to_owned))
        .collect();
    names.sort();
    names
}

/// Identifies a version of the file: its modified time and length, or
/// `"none"` if there's no file.
fn stamp(path: &Path) -> String {
    let Ok(meta) = std::fs::metadata(path) else {
        return "none".to_owned();
    };
    let nanos = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_nanos());
    format!("{nanos}-{}", meta.len())
}

/// Saves the settings: the old file goes to `.bak`, and the new one is
/// written beside it and then moved into place, so Wallflex never reads
/// half a file.
fn write_config(path: &Path, text: &str) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)
            .map_err(|e| format!("can't make {}: {e}", dir.display()))?;
    }
    if path.is_file() {
        let backup = path.with_extension("toml.bak");
        std::fs::copy(path, &backup).map_err(|e| {
            format!("can't back up to {}: {e}", backup.display())
        })?;
    }
    let partial = path.with_extension("toml.new");
    std::fs::write(&partial, text)
        .map_err(|e| format!("can't write {}: {e}", partial.display()))?;
    std::fs::rename(&partial, path)
        .map_err(|e| format!("can't replace {}: {e}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::{new_token, preview_name, stamp, write_config};

    #[test]
    fn accepts_only_plain_preview_names() {
        assert_eq!(preview_name("/api/preview/panel.bmp"), Some("panel"));
        assert_eq!(preview_name("/api/preview/gpu2.bmp"), Some("gpu2"));
        assert_eq!(preview_name("/api/preview/../x.bmp"), None);
        assert_eq!(preview_name("/api/preview/.bmp"), None);
        assert_eq!(preview_name("/api/preview/Panel.bmp"), None);
        assert_eq!(preview_name("/api/preview/panel.txt"), None);
    }

    #[test]
    fn tokens_are_long_and_differ() {
        let (a, b) = (new_token(), new_token());
        assert_eq!(a.len(), 32);
        assert_ne!(a, b);
    }

    #[test]
    fn saving_keeps_a_backup() {
        let dir = std::env::temp_dir()
            .join(format!("wallflex-settings-test-{}", std::process::id()));
        let path = dir.join("config.toml");
        assert_eq!(stamp(&path), "none");
        write_config(&path, "a = 1\n").unwrap();
        let first = stamp(&path);
        write_config(&path, "a = 22\n").unwrap();
        assert_ne!(stamp(&path), first);
        let saved = std::fs::read_to_string(&path).unwrap();
        let backup =
            std::fs::read_to_string(path.with_extension("toml.bak")).unwrap();
        assert_eq!((saved.as_str(), backup.as_str()), ("a = 22\n", "a = 1\n"));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
