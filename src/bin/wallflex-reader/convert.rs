//! Turning HTML and LaTeX guides into Markdown with pandoc.
//!
//! Pandoc is optional: the reader doesn't ship it, it looks for
//! `pandoc.exe` on `PATH` when asked. The converted file is written
//! next to the original in the Guides folder (`Setup.html` becomes
//! `Setup.md`, or `Setup (2).md` if that is taken) and the original is
//! left alone. Pandoc runs with `--sandbox`, so it can read only the
//! file it is given, and is stopped after [`TIME_LIMIT`].

use std::ffi::{OsStr, OsString};
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use crate::guides::{self, Kind};

/// How long pandoc may run before it is stopped.
pub const TIME_LIMIT: Duration = Duration::from_secs(20);
/// How often a running pandoc is checked on.
const POLL: Duration = Duration::from_millis(25);
/// The most of pandoc's complaint kept, in bytes.
const MAX_COMPLAINT_BYTES: u64 = 2048;
/// The most copies of a converted guide (`Name (2).md` ...) tried.
const MAX_COPIES: u32 = 99;
/// The program's file name on `PATH`.
const PANDOC: &str = "pandoc.exe";
/// The folder pandoc's installers use, under `%LOCALAPPDATA%` or
/// `%ProgramFiles%`.
const PANDOC_FOLDER: &str = "Pandoc";
/// Where to get pandoc, for the message when it's missing.
const INSTALL_HINT: &str = "Pandoc isn't installed (or isn't on PATH). \
                            Install it with: winget install \
                            JohnMacFarlane.Pandoc";
/// Process creation flag: no console window.
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// What `pandoc` calls the input format of a guide of `kind`; `None` for
/// kinds that aren't converted.
fn input_format(kind: Kind) -> Option<&'static str> {
    match kind {
        Kind::Html => Some("html"),
        Kind::Tex => Some("latex"),
        Kind::Markdown | Kind::Text | Kind::Pdf => None,
    }
}

/// Whether a guide of `kind` can be converted.
pub fn can_convert(kind: Kind) -> bool {
    input_format(kind).is_some()
}

/// What a conversion will do.
#[derive(Debug, PartialEq, Eq)]
pub struct Plan {
    /// The file to read.
    pub input: PathBuf,
    /// The file to write.
    pub output: PathBuf,
    /// The new guide's name inside the Guides folder.
    pub name: String,
    /// Pandoc's name for the input format.
    pub format: &'static str,
}

/// Works out what converting guide `name` in `folder` involves, or why it
/// can't be done. Nothing is written.
pub fn plan(folder: &Path, name: &str) -> Result<Plan, String> {
    let input = guides::resolve(folder, name)
        .ok_or_else(|| "That isn't a guide.".to_owned())?;
    let format = guides::kind(name)
        .and_then(input_format)
        .ok_or("Only HTML and LaTeX guides can be converted.")?;
    if !input.is_file() {
        return Err(format!("{name} isn't in the Guides folder any more."));
    }
    let stem = name.rsplit_once('.').map_or(name, |(stem, _)| stem);
    let candidates = std::iter::once(format!("{stem}.md"))
        .chain((2..=MAX_COPIES).map(|n| format!("{stem} ({n}).md")));
    for candidate in candidates {
        let Some(output) = guides::resolve(folder, &candidate) else {
            break;
        };
        if !output.exists() {
            return Ok(Plan {
                input,
                output,
                name: candidate,
                format,
            });
        }
    }
    Err("Too many converted copies of this guide already.".to_owned())
}

/// The arguments that make pandoc turn `plan`'s input into GitHub
/// Markdown without raw HTML (which the reader would only show as text)
/// or line wrapping.
fn arguments(plan: &Plan) -> Vec<OsString> {
    let mut args: Vec<OsString> = ["--sandbox", "--wrap=none", "-f"]
        .iter()
        .map(OsString::from)
        .collect();
    args.push(OsString::from(plan.format));
    args.push(OsString::from("-t"));
    args.push(OsString::from("gfm-raw_html"));
    args.push(OsString::from("-o"));
    args.push(plan.output.clone().into_os_string());
    args.push(plan.input.clone().into_os_string());
    args
}

/// Where to look for pandoc, in order: the directories of `path_var`,
/// then pandoc's usual install folders under `local_app_data` (its
/// per-user installer) and `program_files`. The usual folders matter
/// because a program's `PATH` is fixed when it starts: one started
/// before pandoc was installed (such as Explorer, and so everything
/// started at sign-in) never sees the new entry.
pub fn search_dirs(
    path_var: Option<&OsStr>,
    local_app_data: Option<&OsStr>,
    program_files: Option<&OsStr>,
) -> Vec<PathBuf> {
    let on_path = path_var.map(std::env::split_paths).into_iter().flatten();
    let usual = [local_app_data, program_files]
        .into_iter()
        .flatten()
        .map(|base| Path::new(base).join(PANDOC_FOLDER));
    on_path.chain(usual).collect()
}

/// The first `pandoc.exe` in the absolute directories of `dirs`.
/// Relative entries are skipped, so a file in the current directory
/// can't stand in for it.
pub fn find_pandoc(dirs: &[PathBuf]) -> Option<PathBuf> {
    dirs.iter()
        .filter(|dir| dir.is_absolute())
        .map(|dir| dir.join(PANDOC))
        .find(|candidate| candidate.is_file())
}

/// Converts the guide `plan` describes, using `pandoc`. On failure no
/// output file is left behind.
pub fn convert(pandoc: &Path, plan: &Plan) -> Result<(), String> {
    let result = run_limited(pandoc, &arguments(plan), TIME_LIMIT);
    if result.is_err() {
        // A half-written file is worse than none; if there's nothing to
        // remove, there's nothing to clean up.
        let _ = std::fs::remove_file(&plan.output);
    }
    result
}

/// Runs `program` with `args` and no input, stopping it after `limit`.
/// `Err` says why it didn't succeed, with the first line it complained
/// with if there was one.
pub fn run_limited(
    program: &Path,
    args: &[OsString],
    limit: Duration,
) -> Result<(), String> {
    let mut command = Command::new(program);
    command
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    let mut child = command
        .spawn()
        .map_err(|error| format!("Can't start pandoc: {error}"))?;
    // Read on a thread so a chatty pandoc can't fill the pipe and stall.
    let complaint = child.stderr.take().map(|pipe| {
        thread::spawn(move || {
            let mut pipe = pipe;
            let mut kept = Vec::new();
            let _ =
                (&mut pipe).take(MAX_COMPLAINT_BYTES).read_to_end(&mut kept);
            // Whatever is left is read and dropped so pandoc can finish.
            let _ = io::copy(&mut pipe, &mut io::sink());
            String::from_utf8_lossy(&kept).into_owned()
        })
    });
    let started = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if started.elapsed() >= limit => {
                // Already gone is fine; either way it's reaped below.
                let _ = child.kill();
                let _ = child.wait();
                return Err("Pandoc took too long and was stopped.".to_owned());
            }
            Ok(None) => thread::sleep(POLL),
            Err(error) => return Err(format!("Pandoc stopped: {error}")),
        }
    };
    if status.success() {
        return Ok(());
    }
    let said = complaint
        .and_then(|handle| handle.join().ok())
        .unwrap_or_default();
    let first = said.lines().find(|line| !line.trim().is_empty());
    Err(first.map_or_else(
        || format!("Pandoc failed ({status})."),
        |line| format!("Pandoc failed: {}", line.trim()),
    ))
}

/// Converts guide `name` in `folder`, finding pandoc in `dirs` (see
/// [`search_dirs`]). Returns the new guide's name.
pub fn convert_guide(
    folder: &Path,
    name: &str,
    dirs: &[PathBuf],
) -> Result<String, String> {
    let plan = plan(folder, name)?;
    let pandoc = find_pandoc(dirs).ok_or_else(|| INSTALL_HINT.to_owned())?;
    convert(&pandoc, &plan)?;
    Ok(plan.name)
}

#[cfg(test)]
mod tests {
    use std::ffi::{OsStr, OsString};
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::time::Duration;

    use super::{
        Plan, arguments, can_convert, convert_guide, find_pandoc, plan,
        run_limited, search_dirs,
    };
    use crate::guides::Kind;

    /// A fresh empty folder for one test.
    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir()
            .join(format!("wallflex-convert-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn converts_only_html_and_latex() {
        assert!(can_convert(Kind::Html));
        assert!(can_convert(Kind::Tex));
        assert!(!can_convert(Kind::Markdown));
        assert!(!can_convert(Kind::Text));
        assert!(!can_convert(Kind::Pdf));
    }

    #[test]
    fn plans_a_sibling_markdown_file() {
        let dir = scratch("plan");
        fs::create_dir_all(dir.join("Games")).unwrap();
        fs::write(dir.join("Games").join("Setup.html"), "<p>x</p>").unwrap();
        let first = plan(&dir, "Games/Setup.html").unwrap();
        assert_eq!(first.name, "Games/Setup.md");
        assert_eq!(first.format, "html");
        fs::write(&first.output, "taken").unwrap();
        let second = plan(&dir, "Games/Setup.html").unwrap();
        assert_eq!(second.name, "Games/Setup (2).md");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn plans_latex_and_refuses_the_rest() {
        let dir = scratch("kinds");
        fs::write(dir.join("p.tex"), "x").unwrap();
        fs::write(dir.join("a.md"), "x").unwrap();
        assert_eq!(plan(&dir, "p.tex").unwrap().format, "latex");
        assert!(plan(&dir, "a.md").is_err());
        assert!(plan(&dir, "missing.html").is_err());
        assert!(plan(&dir, "../p.tex").is_err());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn asks_pandoc_for_sandboxed_markdown() {
        let plan = Plan {
            input: PathBuf::from("in.html"),
            output: PathBuf::from("out.md"),
            name: "out.md".to_owned(),
            format: "html",
        };
        let args = arguments(&plan);
        assert_eq!(args.first(), Some(&OsString::from("--sandbox")));
        assert_eq!(args.last(), Some(&OsString::from("in.html")));
        assert!(args.contains(&OsString::from("gfm-raw_html")));
    }

    #[test]
    fn finds_pandoc_only_in_absolute_directories() {
        let dir = scratch("path");
        fs::write(dir.join("pandoc.exe"), "").unwrap();
        let other = Path::new("somewhere-else").to_path_buf();
        let both = [other.clone(), dir.clone()];
        assert_eq!(find_pandoc(&both), Some(dir.join("pandoc.exe")));
        assert_eq!(find_pandoc(&[other]), None);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn searches_path_then_the_usual_install_folders() {
        let path = std::env::join_paths([r"C:\a", r"C:\b"]).unwrap();
        let dirs = search_dirs(
            Some(&path),
            Some(OsStr::new(r"C:\Users\x\AppData\Local")),
            Some(OsStr::new(r"C:\Program Files")),
        );
        assert_eq!(
            dirs,
            [
                PathBuf::from(r"C:\a"),
                PathBuf::from(r"C:\b"),
                PathBuf::from(r"C:\Users\x\AppData\Local\Pandoc"),
                PathBuf::from(r"C:\Program Files\Pandoc"),
            ]
        );
        assert_eq!(search_dirs(None, None, None).len(), 0);
    }

    #[test]
    fn says_so_when_pandoc_is_missing() {
        let dir = scratch("missing");
        fs::write(dir.join("a.html"), "<p>x</p>").unwrap();
        let error = convert_guide(&dir, "a.html", std::slice::from_ref(&dir))
            .unwrap_err();
        assert!(error.contains("winget"));
        let none = convert_guide(&dir, "a.html", &[]).unwrap_err();
        assert!(none.contains("winget"));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn converts_with_real_pandoc_and_never_overwrites() {
        let dirs = search_dirs(
            std::env::var_os("PATH").as_deref(),
            std::env::var_os("LOCALAPPDATA").as_deref(),
            std::env::var_os("ProgramFiles").as_deref(),
        );
        if find_pandoc(&dirs).is_none() {
            // Pandoc is optional; there's nothing to try without it.
            return;
        }
        let dir = scratch("real");
        fs::write(dir.join("Page.html"), "<h1>Hi</h1><p>x</p>").unwrap();
        fs::write(dir.join("Page.md"), "mine").unwrap();
        let first = convert_guide(&dir, "Page.html", &dirs).unwrap();
        assert_eq!(first, "Page (2).md");
        let second = convert_guide(&dir, "Page.html", &dirs).unwrap();
        assert_eq!(second, "Page (3).md");
        assert_eq!(fs::read_to_string(dir.join("Page.md")).unwrap(), "mine");
        let text = fs::read_to_string(dir.join("Page (2).md")).unwrap();
        assert!(text.contains("# Hi"), "{text}");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn reports_a_failing_program() {
        let cmd = Path::new(r"C:\Windows\System32\cmd.exe");
        let args = ["/c", "echo boom 1>&2 & exit 3"].map(OsString::from);
        let error =
            run_limited(cmd, &args, Duration::from_secs(10)).unwrap_err();
        assert!(error.contains("boom"), "{error}");
        let fine = ["/c", "exit 0"].map(OsString::from);
        assert!(run_limited(cmd, &fine, Duration::from_secs(10)).is_ok());
    }

    #[cfg(windows)]
    #[test]
    fn stops_a_program_that_runs_too_long() {
        let ping = Path::new(r"C:\Windows\System32\ping.exe");
        let args = ["-n", "30", "127.0.0.1"].map(OsString::from);
        let error =
            run_limited(ping, &args, Duration::from_millis(200)).unwrap_err();
        assert!(error.contains("too long"), "{error}");
    }
}
