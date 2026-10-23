//! Command-line options.

use std::fmt;

use crate::color::Rgb;

/// Help text shown by `--help` and after a usage error.
pub const USAGE: &str = "\
Usage: wallflex [options]

Draws CPU, network and GPU widgets on a solid background behind the
desktop icons, laid out by %APPDATA%\\Wallflex\\config.toml.

Options:
  --color <RRGGBB>  background colour, overriding the config file
  --config <FILE>   use this settings file instead
  --sensors         print one sensor reading, then exit
  --preview <HEIGHT> <FILE>
                    save the widgets as a BMP, as they'd look on a
                    monitor HEIGHT pixels tall, then exit
  --diagnose        print how the desktop windows are laid out, then exit
  --quit            stop the running copy of Wallflex
  -h, --help        show this help";

/// What the program was asked to do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Command {
    /// Draw until told to quit.
    Run,
    /// Ask a running copy to quit.
    Quit,
    /// Print the desktop window layout.
    Diagnose,
    /// Print one sensor reading.
    Sensors,
    /// Save the widgets as a BMP.
    Preview {
        /// Monitor height to render for, in pixels.
        height: u32,
        /// Where to save the BMP.
        path: String,
    },
    /// Print the help text.
    Help,
}

/// Everything parsed from the command line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Options {
    /// What to do.
    pub command: Command,
    /// Background colour from `--color`, overriding the config file.
    pub background: Option<Rgb>,
    /// Settings file from `--config`, instead of the default one.
    pub config: Option<String>,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            command: Command::Run,
            background: None,
            config: None,
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
            "--color" => {
                let value =
                    args.next().ok_or(ArgError::MissingValue("--color"))?;
                let color =
                    Rgb::from_hex(&value).ok_or(ArgError::BadValue {
                        option: "--color",
                        value,
                    })?;
                options.background = Some(color);
            }
            "--config" => {
                let value =
                    args.next().ok_or(ArgError::MissingValue("--config"))?;
                options.config = Some(value);
            }
            "--sensors" => options.command = Command::Sensors,
            "--preview" => {
                let height =
                    args.next().ok_or(ArgError::MissingValue("--preview"))?;
                let path =
                    args.next().ok_or(ArgError::MissingValue("--preview"))?;
                let height =
                    height.parse::<u32>().ok().filter(|h| *h > 0).ok_or(
                        ArgError::BadValue {
                            option: "--preview",
                            value: height,
                        },
                    )?;
                options.command = Command::Preview { height, path };
            }
            "--diagnose" => options.command = Command::Diagnose,
            "--quit" => options.command = Command::Quit,
            "-h" | "--help" => options.command = Command::Help,
            _ => return Err(ArgError::Unknown(arg)),
        }
    }
    Ok(options)
}

#[cfg(test)]
mod tests {
    use super::{ArgError, Command, Options, parse};
    use crate::color::Rgb;

    fn run(args: &[&str]) -> Result<Options, ArgError> {
        parse(args.iter().map(|a| (*a).to_owned()))
    }

    #[test]
    fn no_arguments_gives_defaults() {
        assert_eq!(run(&[]), Ok(Options::default()));
    }

    #[test]
    fn reads_color() {
        assert_eq!(
            run(&["--color", "#ff0000"]),
            Ok(Options {
                command: Command::Run,
                background: Some(Rgb { r: 255, g: 0, b: 0 }),
                config: None,
            })
        );
    }

    #[test]
    fn reads_commands() {
        let command = |args| run(args).map(|o| o.command);
        assert_eq!(command(&["--quit"]), Ok(Command::Quit));
        assert_eq!(command(&["--diagnose"]), Ok(Command::Diagnose));
        assert_eq!(command(&["--sensors"]), Ok(Command::Sensors));
        assert_eq!(
            command(&["--preview", "2160", "a.bmp"]),
            Ok(Command::Preview {
                height: 2160,
                path: "a.bmp".to_owned()
            })
        );
        assert!(command(&["--preview", "0", "a.bmp"]).is_err());
        assert!(command(&["--preview", "1080"]).is_err());
        assert_eq!(command(&["-h"]), Ok(Command::Help));
    }

    #[test]
    fn reports_errors() {
        assert_eq!(
            run(&["--bogus"]),
            Err(ArgError::Unknown("--bogus".to_owned()))
        );
        assert_eq!(run(&["--color"]), Err(ArgError::MissingValue("--color")));
        assert_eq!(
            run(&["--color", "red"]),
            Err(ArgError::BadValue {
                option: "--color",
                value: "red".to_owned(),
            })
        );
    }
}
