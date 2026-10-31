//! The reading style: colour theme, font, text size and line spacing,
//! chosen in the page's "Aa" panel and remembered in `reader.json`.
//!
//! The page owns how each choice looks (its CSS); this only knows the
//! names, so anything the page sends is checked against them before
//! it's saved.

use serde::{Deserialize, Serialize};

/// Colour themes: `auto` follows Windows' light or dark mode.
pub const THEMES: [&str; 5] = ["auto", "light", "sepia", "dark", "black"];
/// Fonts, by the page's names for them.
pub const FONTS: [&str; 6] = [
    "segoe",
    "sitka",
    "georgia",
    "verdana",
    "bahnschrift",
    "cascadia",
];
/// Line spacing choices.
pub const SPACINGS: [&str; 3] = ["compact", "normal", "airy"];
/// The smallest and largest text size, in CSS pixels.
pub const SIZES: std::ops::RangeInclusive<u32> = 8..=28;
/// The text size to start with.
const DEFAULT_SIZE: u32 = 16;

/// How guides look.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Style {
    /// One of [`THEMES`].
    pub theme: String,
    /// One of [`FONTS`].
    pub font: String,
    /// Text size in CSS pixels, within [`SIZES`].
    pub size: u32,
    /// One of [`SPACINGS`].
    pub spacing: String,
}

impl Default for Style {
    fn default() -> Self {
        Self {
            theme: "auto".to_owned(),
            font: "segoe".to_owned(),
            size: DEFAULT_SIZE,
            spacing: "normal".to_owned(),
        }
    }
}

impl Style {
    /// The style with anything unknown put back to its default and the
    /// size kept in range.
    #[must_use]
    pub fn checked(self) -> Self {
        let default = Self::default();
        let pick = |value: String, known: &[&str], fallback: String| {
            if known.contains(&value.as_str()) {
                value
            } else {
                fallback
            }
        };
        Self {
            theme: pick(self.theme, &THEMES, default.theme),
            font: pick(self.font, &FONTS, default.font),
            size: self.size.clamp(*SIZES.start(), *SIZES.end()),
            spacing: pick(self.spacing, &SPACINGS, default.spacing),
        }
    }

    /// The window's colour behind the page (RGB) and whether its frame
    /// should be dark, for this theme; `dark_mode` is Windows' setting,
    /// which `auto` follows.
    pub fn background(&self, dark_mode: bool) -> ([u8; 3], bool) {
        match self.theme.as_str() {
            "light" => (LIGHT, false),
            "sepia" => (SEPIA, false),
            "dark" => (DARK, true),
            "black" => (BLACK, true),
            _ if dark_mode => (DARK, true),
            _ => (LIGHT, false),
        }
    }
}

/// The light theme's background (`#fbfaf7`).
const LIGHT: [u8; 3] = [0xFB, 0xFA, 0xF7];
/// The sepia theme's background (`#f4ecd8`).
const SEPIA: [u8; 3] = [0xF4, 0xEC, 0xD8];
/// The dark theme's background (`#141416`).
const DARK: [u8; 3] = [0x14, 0x14, 0x16];
/// The black theme's background.
const BLACK: [u8; 3] = [0, 0, 0];

#[cfg(test)]
mod tests {
    use super::Style;

    #[test]
    fn keeps_known_choices() {
        let style = Style {
            theme: "sepia".to_owned(),
            font: "sitka".to_owned(),
            size: 20,
            spacing: "airy".to_owned(),
        };
        assert_eq!(style.clone().checked(), style);
    }

    #[test]
    fn fixes_unknown_choices() {
        let style = Style {
            theme: "neon".to_owned(),
            font: "url(evil)".to_owned(),
            size: 400,
            spacing: String::new(),
        }
        .checked();
        assert_eq!(style.theme, "auto");
        assert_eq!(style.font, "segoe");
        assert_eq!(style.size, 28);
        assert_eq!(style.spacing, "normal");
        assert_eq!(
            Style {
                size: 1,
                ..Style::default()
            }
            .checked()
            .size,
            8
        );
    }

    #[test]
    fn picks_window_colours() {
        let sepia = Style {
            theme: "sepia".to_owned(),
            ..Style::default()
        };
        assert_eq!(sepia.background(true), ([0xF4, 0xEC, 0xD8], false));
        assert!(Style::default().background(true).1);
        assert!(!Style::default().background(false).1);
    }
}
