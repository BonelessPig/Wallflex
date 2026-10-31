//! Web pages in reader mode.
//!
//! The app only fetches: it runs the `curl.exe` that ships with Windows
//! (by its full path, so nothing on `PATH` can stand in for it) on its
//! own thread and hands the page's HTML to the reader page. The page
//! cuts out the article and cleans it (`shell.html`, in an inert
//! `DOMParser` document), and it is shown in the same script-free
//! reading frame as a guide. Links in it come back as
//! `https://read.wallflex/?u=ADDRESS`, which [`read_target`] turns into
//! the address to fetch next.

use std::ffi::OsString;
use std::io::Read;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use crate::guides;

/// The address links inside a web reading page are given; the browser
/// is stopped before it goes there.
const READ_BASE: &str = "https://read.wallflex/";
/// The link that goes back to the page before.
const BACK_URL: &str = "https://read.wallflex/?back";
/// How long a download may take, in seconds.
const TIME_LIMIT_SECS: u64 = 20;
/// The biggest page taken, in bytes.
const MAX_PAGE_BYTES: u64 = 8 * 1024 * 1024;
/// The most redirects followed.
const MAX_REDIRECTS: u32 = 5;
/// The longest address accepted, in bytes.
const MAX_URL_BYTES: usize = 2048;
/// How often a running download is checked on.
const POLL: Duration = Duration::from_millis(25);
/// The most of curl's complaint kept, in bytes.
const MAX_COMPLAINT_BYTES: u64 = 2048;
/// Pretends to be a browser, since some sites refuse anything else.
const USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) \
                          AppleWebKit/537.36 (KHTML, like Gecko) \
                          Chrome/130.0 Safari/537.36 WallflexReader";
/// Process creation flag: no console window.
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;
/// Host name endings that mean this PC or its network (the host is
/// already lower case).
const LOCAL_SUFFIXES: [&str; 2] = [".local", ".localhost"];
/// The second octets of the private range 172.16.0.0/12.
const PRIVATE_172: std::ops::RangeInclusive<u8> = 16..=31;

/// A page that was fetched.
#[derive(Debug, PartialEq, Eq)]
pub struct Page {
    /// Where it ended up, after redirects.
    pub url: String,
    /// Its HTML.
    pub html: String,
}

/// The address `input` stands for, or `None` if it isn't a web address
/// the reader will fetch. A missing scheme is taken to be `https://`.
/// Credentials in the address, other schemes and local or private hosts
/// are refused.
pub fn normalize(input: &str) -> Option<String> {
    let text = input.trim();
    let lower = text.to_ascii_lowercase();
    let full = if lower.starts_with("https://") || lower.starts_with("http://")
    {
        text.to_owned()
    } else if text.contains("://") || text.starts_with("mailto:") {
        return None;
    } else {
        format!("https://{text}")
    };
    let bad = full.len() > MAX_URL_BYTES
        || full.chars().any(|c| c.is_control() || c.is_whitespace());
    if bad {
        return None;
    }
    let rest = full.split_once("://")?.1;
    let authority = rest.split(['/', '?', '#']).next()?;
    let host = authority_host(authority)?;
    (host.contains('.') && !is_private(&host)).then_some(full)
}

/// The host in `authority` (`host`, `host:port`), lower case; `None` if
/// it has credentials or no host.
fn authority_host(authority: &str) -> Option<String> {
    if authority.contains('@') || authority.is_empty() {
        return None;
    }
    let host = if authority.starts_with('[') {
        authority
    } else {
        authority.split(':').next()?
    };
    (!host.is_empty()).then(|| host.to_ascii_lowercase())
}

/// Whether `host` is this PC or a private network address: the reader
/// shouldn't be steered into fetching from those by a link.
fn is_private(host: &str) -> bool {
    if host.starts_with('[') || host == "localhost" {
        return true;
    }
    if LOCAL_SUFFIXES.iter().any(|suffix| host.ends_with(suffix)) {
        return true;
    }
    let octets: Vec<u8> = host
        .split('.')
        .map_while(|part| part.parse::<u8>().ok())
        .collect();
    if octets.len() != 4 {
        return false;
    }
    match octets.as_slice() {
        [127 | 10 | 0, ..] | [192, 168, ..] | [169, 254, ..] => true,
        [172, second, ..] => PRIVATE_172.contains(second),
        _ => false,
    }
}

/// The address a reading page's link asks for, or `None` if `uri`
/// isn't one (or asks for something the reader won't fetch).
pub fn read_target(uri: &str) -> Option<String> {
    let query = uri.strip_prefix(READ_BASE)?.strip_prefix("?u=")?;
    let query = query.split('#').next().unwrap_or_default();
    normalize(&guides::percent_decode(query)?)
}

/// Whether `uri` is the reading pages' "back" link.
pub fn is_back(uri: &str) -> bool {
    uri.strip_prefix(BACK_URL).is_some_and(|rest| {
        rest.is_empty() || rest.starts_with('#') || rest.starts_with('&')
    })
}

/// Where the system's `curl.exe` is, if this Windows has one.
fn curl_path(system_root: Option<OsString>) -> Option<PathBuf> {
    let path = PathBuf::from(system_root?)
        .join("System32")
        .join("curl.exe");
    path.is_file().then_some(path)
}

/// curl's arguments for downloading `url`: follow a few redirects over
/// http(s) only, give up after the time limit or a big page, and print
/// the final address and content type on stderr after the page.
fn arguments(url: &str) -> Vec<OsString> {
    let mut args: Vec<OsString> = [
        "--silent",
        "--show-error",
        "--location",
        "--compressed",
        "--proto",
        "=http,https",
        "--proto-redir",
        "=http,https",
        "--globoff",
    ]
    .iter()
    .map(OsString::from)
    .collect();
    let numbers = [
        ("--max-redirs", MAX_REDIRECTS.to_string()),
        ("--max-time", TIME_LIMIT_SECS.to_string()),
        ("--max-filesize", MAX_PAGE_BYTES.to_string()),
    ];
    for (flag, value) in numbers {
        args.push(OsString::from(flag));
        args.push(OsString::from(value));
    }
    for (flag, value) in [
        ("--user-agent", USER_AGENT),
        ("--header", "Accept: text/html,application/xhtml+xml"),
        (
            "--write-out",
            "%{stderr}\n%{url_effective}\n%{content_type}",
        ),
    ] {
        args.push(OsString::from(flag));
        args.push(OsString::from(value));
    }
    args.push(OsString::from("--"));
    args.push(OsString::from(url));
    args
}

/// What curl said after the page: its final address and content type,
/// from the last two lines of stderr.
fn trailer(stderr: &str) -> Option<(String, String)> {
    let mut lines = stderr.lines().rev();
    let kind = lines.next()?;
    let url = lines.next()?;
    Some((url.trim().to_owned(), kind.trim().to_ascii_lowercase()))
}

/// Downloads `url` (already passed through [`normalize`]).
pub fn fetch(url: &str, system_root: Option<OsString>) -> Result<Page, String> {
    let curl = curl_path(system_root)
        .ok_or("This Windows has no curl.exe, so pages can't be fetched.")?;
    let mut command = Command::new(curl);
    command
        .args(arguments(url))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    let mut child = command
        .spawn()
        .map_err(|error| format!("Can't start curl: {error}"))?;
    let body = child.stdout.take().map(|pipe| {
        thread::spawn(move || {
            let mut kept = Vec::new();
            let _ = pipe.take(MAX_PAGE_BYTES).read_to_end(&mut kept);
            kept
        })
    });
    let said = child.stderr.take().map(|pipe| {
        thread::spawn(move || {
            let mut kept = Vec::new();
            let _ = pipe.take(MAX_COMPLAINT_BYTES).read_to_end(&mut kept);
            String::from_utf8_lossy(&kept).into_owned()
        })
    });
    let started = Instant::now();
    let limit = Duration::from_secs(TIME_LIMIT_SECS.saturating_add(2));
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if started.elapsed() >= limit => {
                let _ = child.kill();
                let _ = child.wait();
                return Err("The page took too long to load.".to_owned());
            }
            Ok(None) => thread::sleep(POLL),
            Err(error) => return Err(format!("curl stopped: {error}")),
        }
    };
    let said = said
        .and_then(|handle| handle.join().ok())
        .unwrap_or_default();
    let bytes = body
        .and_then(|handle| handle.join().ok())
        .unwrap_or_default();
    finish(status.success(), &said, &bytes)
}

/// Turns what curl produced into a page, or says why there isn't one.
fn finish(ok: bool, said: &str, bytes: &[u8]) -> Result<Page, String> {
    if !ok {
        let first = said.lines().find(|line| !line.trim().is_empty());
        return Err(first.map_or_else(
            || "The page couldn't be loaded.".to_owned(),
            |line| format!("The page couldn't be loaded: {}", line.trim()),
        ));
    }
    let (url, kind) = trailer(said).ok_or("curl gave no details.")?;
    let is_html = kind.is_empty()
        || kind.contains("html")
        || kind.starts_with("text/plain");
    if !is_html {
        return Err(format!("That isn't a web page ({kind})."));
    }
    // The final address must pass the same checks as the first.
    let url = normalize(&url).ok_or("The page moved somewhere not allowed.")?;
    let text = String::from_utf8_lossy(bytes);
    let html = text.strip_prefix('\u{feff}').unwrap_or(&text).to_owned();
    Ok(Page { url, html })
}

#[cfg(test)]
mod tests {
    use super::{
        Page, arguments, finish, is_back, normalize, read_target, trailer,
    };

    #[test]
    fn accepts_web_addresses() {
        let wiki = "https://oldschool.runescape.wiki/w/Abyssal_whip";
        assert_eq!(normalize(wiki).as_deref(), Some(wiki));
        assert_eq!(
            normalize("  oldschool.runescape.wiki/w/Cow ").as_deref(),
            Some("https://oldschool.runescape.wiki/w/Cow")
        );
        assert_eq!(
            normalize("http://example.com:8080/a?b=c#d").as_deref(),
            Some("http://example.com:8080/a?b=c#d")
        );
    }

    #[test]
    fn refuses_other_addresses() {
        for bad in [
            "",
            "ftp://example.com/",
            "file:///C:/x",
            "javascript:alert(1)",
            "mailto:a@b.co",
            "https://user:pw@example.com/",
            "https://localhost/",
            "https://example/",
            "http://127.0.0.1:9000/",
            "http://10.1.2.3/",
            "http://192.168.0.1/",
            "http://172.20.0.1/",
            "http://169.254.1.1/",
            "https://[::1]/",
            "https://printer.local/",
            "https://exa mple.com/",
        ] {
            assert_eq!(normalize(bad), None, "{bad}");
        }
        assert!(normalize("http://172.32.0.1/").is_some());
        let long = format!("https://example.com/{}", "a".repeat(3000));
        assert_eq!(normalize(&long), None);
    }

    #[test]
    fn reads_link_targets() {
        assert_eq!(
            read_target(
                "https://read.wallflex/?u=https%3A%2F%2Fa.org%2Fb%3Fc%3D1"
            ),
            Some("https://a.org/b?c=1".to_owned())
        );
        assert_eq!(
            read_target("https://read.wallflex/?u=http%3A%2F%2Flocalhost"),
            None
        );
        assert_eq!(read_target("https://read.wallflex/?u=%zz"), None);
        assert_eq!(read_target("https://read.wallflex/"), None);
        assert_eq!(read_target("https://a.org/?u=x"), None);
        assert!(is_back("https://read.wallflex/?back"));
        assert!(!is_back("https://read.wallflex/?backwards"));
        assert!(!is_back("https://a.org/?back"));
    }

    #[test]
    fn curl_gets_the_address_last_and_after_a_separator() {
        let args = arguments("https://a.org/-x");
        let n = args.len();
        assert_eq!(args[n - 2], "--");
        assert_eq!(args[n - 1], "https://a.org/-x");
    }

    #[test]
    fn reads_curls_trailer() {
        assert_eq!(
            trailer("\nhttps://a.org/x\nText/HTML; charset=utf-8"),
            Some((
                "https://a.org/x".to_owned(),
                "text/html; charset=utf-8".to_owned()
            ))
        );
        assert_eq!(trailer(""), None);
    }

    #[test]
    fn finishes_pages_and_failures() {
        let said = "\nhttps://a.org/x\ntext/html";
        assert_eq!(
            finish(true, said, b"\xEF\xBB\xBF<p>hi"),
            Ok(Page {
                url: "https://a.org/x".to_owned(),
                html: "<p>hi".to_owned()
            })
        );
        let pdf = "\nhttps://a.org/x\napplication/pdf";
        assert!(finish(true, pdf, b"%PDF").unwrap_err().contains("pdf"));
        let moved = "\nhttp://127.0.0.1/x\ntext/html";
        assert!(finish(true, moved, b"x").is_err());
        let failed = "curl: (6) Could not resolve host: nope.example\n";
        assert!(finish(false, failed, b"").unwrap_err().contains("resolve"));
    }
}
