//! Command-line options.

use std::fmt;

use crate::color::Rgb;

/// Help text shown by `--help` and after a usage error.
pub const USAGE: &str = "\
Usage: wallflex [options]

Draws a solid background behind the desktop icons on every monitor.

Options:
  --render <mode>   gdi (default), d2d-hw (Direct2D on the GPU)
                    or d2d-sw (Direct2D on the CPU)
  --color <RRGGBB>  background colour (default 000000)
  --diagnose        print how the desktop windows are laid out, then exit
  --quit            stop the running copy of Wallflex
  -h, --help        show this help";

/// What the program was asked to do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Command {
    /// Draw the background until told to quit.
    Run,
    /// Ask a running copy to quit.
    Quit,
    /// Print the desktop window layout.
    Diagnose,
    /// Print the help text.
    Help,
}

/// How the background is painted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RenderMode {
    /// A plain GDI fill. Loads no graphics driver.
    Gdi,
    /// Direct2D on the GPU.
    D2dHardware,
    /// Direct2D on the CPU.
    D2dSoftware,
}

/// Everything parsed from the command line.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Options {
    /// What to do.
    pub command: Command,
    /// How to paint.
    pub render: RenderMode,
    /// Background colour.
    pub background: Rgb,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            command: Command::Run,
            render: RenderMode::Gdi,
            background: Rgb::BLACK,
        }
    }
}

/// Why the command line could not be parsed.
#[derive(Debug, PartialEq, Eq)]
pub enum ArgError {
    /// An argument that isn't an option Wallflex knows.
    Unknown(String),
    /// An option that needs a value was last on the line.
    MissingValue(&'static str),
    /// An option's value isn't valid.
    BadValue {
        /// The option.
        option: &'static str,
        /// The value given.
        value: String,
    },
}

impl fmt::Display for ArgError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unknown(arg) => write!(f, "unknown argument '{arg}'"),
            Self::MissingValue(option) => {
                write!(f, "{option} needs a value")
            }
            Self::BadValue { option, value } => {
                write!(f, "'{value}' is not a valid value for {option}")
            }
        }
    }
}

impl std::error::Error for ArgError {}

/// Parses the arguments that follow the program name.
///
/// # Errors
///
/// Returns an [`ArgError`] for an unknown option, a missing value or a
/// value that doesn't parse.
pub fn parse<I>(args: I) -> Result<Options, ArgError>
where
    I: IntoIterator<Item = String>,
{
    let mut options = Options::default();
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--render" => {
                let value = value_for("--render", args.next())?;
                options.render =
                    parse_render(&value).ok_or(ArgError::BadValue {
                        option: "--render",
                        value,
                    })?;
            }
            "--color" => {
                let value = value_for("--color", args.next())?;
                options.background =
                    Rgb::from_hex(&value).ok_or(ArgError::BadValue {
                        option: "--color",
                        value,
                    })?;
            }
            "--diagnose" => options.command = Command::Diagnose,
            "--quit" => options.command = Command::Quit,
            "-h" | "--help" => options.command = Command::Help,
            _ => return Err(ArgError::Unknown(arg)),
        }
    }
    Ok(options)
}

/// Returns the option's value, or an error when it's missing.
fn value_for(
    option: &'static str,
    value: Option<String>,
) -> Result<String, ArgError> {
    value.ok_or(ArgError::MissingValue(option))
}

/// Maps a `--render` value to its mode.
fn parse_render(value: &str) -> Option<RenderMode> {
    match value {
        "gdi" => Some(RenderMode::Gdi),
        "d2d-hw" => Some(RenderMode::D2dHardware),
        "d2d-sw" => Some(RenderMode::D2dSoftware),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::{ArgError, Command, Options, RenderMode, parse};
    use crate::color::Rgb;

    fn run(args: &[&str]) -> Result<Options, ArgError> {
        parse(args.iter().map(|a| (*a).to_owned()))
    }

    #[test]
    fn no_arguments_gives_defaults() {
        assert_eq!(run(&[]), Ok(Options::default()));
    }

    #[test]
    fn reads_render_and_color() {
        let options = run(&["--render", "d2d-sw", "--color", "#ff0000"]);
        assert_eq!(
            options,
            Ok(Options {
                command: Command::Run,
                render: RenderMode::D2dSoftware,
                background: Rgb { r: 255, g: 0, b: 0 },
            })
        );
    }

    #[test]
    fn reads_commands() {
        let command = |args| run(args).map(|o| o.command);
        assert_eq!(command(&["--quit"]), Ok(Command::Quit));
        assert_eq!(command(&["--diagnose"]), Ok(Command::Diagnose));
        assert_eq!(command(&["-h"]), Ok(Command::Help));
    }

    #[test]
    fn reports_errors() {
        assert_eq!(
            run(&["--bogus"]),
            Err(ArgError::Unknown("--bogus".to_owned()))
        );
        assert_eq!(run(&["--render"]), Err(ArgError::MissingValue("--render")));
        assert_eq!(
            run(&["--render", "vulkan"]),
            Err(ArgError::BadValue {
                option: "--render",
                value: "vulkan".to_owned(),
            })
        );
    }
}
