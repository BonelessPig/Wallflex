//! The reader's command line.

use std::path::PathBuf;

/// Help text for `--help` and usage errors.
pub const USAGE: &str = "\
Usage: wallflex-reader [options]

Shows your guides (Markdown, HTML, PDF and text files in the Guides
folder next to Wallflex's settings) in a borderless window. Wallflex's
book icon starts it; starting it again shows or hides the window.

Options:
  --at <X,Y>       open next to this point (desktop pixels), as the
                   book icon does
  --config <FILE>  Wallflex's settings file, to find the Guides folder
                   next to it (default %APPDATA%\\Wallflex\\config.toml)
  --quit           close a running reader (exit code 0 if there was one)
  -h, --help       show this help";

/// Command-line options.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Options {
    /// Where the book icon was clicked, if given.
    pub at: Option<(i32, i32)>,
    /// Wallflex's settings file, from `--config`.
    pub config: Option<PathBuf>,
    /// Close the running copy instead of starting one.
    pub quit: bool,
    /// Show the help.
    pub help: bool,
}

/// Parses the arguments after the program name.
///
/// # Errors
///
/// A message saying which argument is wrong.
pub fn parse<I: IntoIterator<Item = String>>(
    args: I,
) -> Result<Options, String> {
    let mut options = Options::default();
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--at" => {
                let value = args.next().ok_or("--at needs X,Y")?;
                options.at = Some(
                    parse_point(&value)
                        .ok_or_else(|| format!("--at: '{value}' isn't X,Y"))?,
                );
            }
            "--config" => {
                let file = args.next().ok_or("--config needs a file")?;
                options.config = Some(PathBuf::from(file));
            }
            "--quit" => options.quit = true,
            "-h" | "--help" => options.help = true,
            other => return Err(format!("unknown argument '{other}'")),
        }
    }
    Ok(options)
}

/// Parses `X,Y` (whole pixels, either may be negative).
pub fn parse_point(text: &str) -> Option<(i32, i32)> {
    let (x, y) = text.split_once(',')?;
    Some((x.trim().parse().ok()?, y.trim().parse().ok()?))
}

#[cfg(test)]
mod tests {
    use super::{Options, parse, parse_point};
    use std::path::PathBuf;

    fn run(args: &[&str]) -> Result<Options, String> {
        parse(args.iter().map(|a| (*a).to_owned()))
    }

    #[test]
    fn reads_options() {
        assert_eq!(run(&[]), Ok(Options::default()));
        let options = run(&["--at", "-1920,1000", "--config", "a.toml"]);
        assert_eq!(
            options,
            Ok(Options {
                at: Some((-1920, 1000)),
                config: Some(PathBuf::from("a.toml")),
                quit: false,
                help: false,
            })
        );
        assert!(run(&["--help"]).unwrap().help);
        assert!(run(&["--quit"]).unwrap().quit);
    }

    #[test]
    fn rejects_bad_arguments() {
        assert!(run(&["--at"]).is_err());
        assert!(run(&["--at", "12"]).is_err());
        assert!(run(&["--config"]).is_err());
        assert!(run(&["--bogus"]).is_err());
    }

    #[test]
    fn reads_points() {
        assert_eq!(parse_point("3, -4"), Some((3, -4)));
        assert_eq!(parse_point("3;4"), None);
        assert_eq!(parse_point("x,4"), None);
    }
}
