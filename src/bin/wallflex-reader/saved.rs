//! What the reader remembers between runs, in `reader.json` next to
//! Wallflex's settings: whether it's pinned and to which side, the last
//! guide shown, the sizes it was resized to, where the floating panel
//! was dragged to, how far the page is zoomed
//! (Ctrl and the mouse wheel), the reading style (theme, font, size,
//! spacing) and the corner the menu dot sits in.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::corner::Corner;
use crate::place::Edge;
use crate::style::Style;

/// The file's name, next to the settings file.
pub const FILE: &str = "reader.json";

/// What's remembered.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Saved {
    /// Docked as a pane rather than floating.
    pub pinned: bool,
    /// The side it docks against; `None` until one is picked, meaning
    /// the side nearer the book icon.
    pub side: Option<Edge>,
    /// The last guide shown, by name.
    pub last: Option<String>,
    /// The floating panel's size, as on a 1080p monitor at 100%
    /// scaling; `None` for the usual size.
    pub panel: Option<(i32, i32)>,
    /// The docked pane's width, likewise.
    pub pane: Option<i32>,
    /// Where the floating panel's top-left corner was dragged to, in
    /// desktop pixels; `None` for the corner nearest the book icon.
    pub spot: Option<(i32, i32)>,
    /// The page's zoom on top of the usual size (1 = as usual).
    pub zoom: f64,
    /// How guides look.
    pub style: Style,
    /// The corner the menu dot sits in.
    pub corner: Corner,
    /// The contents list is pinned in the page's left margin.
    pub contents: bool,
}

impl Default for Saved {
    fn default() -> Self {
        Self {
            pinned: false,
            side: None,
            last: None,
            panel: None,
            pane: None,
            spot: None,
            zoom: 1.0,
            style: Style::default(),
            corner: Corner::default(),
            contents: false,
        }
    }
}

impl Saved {
    /// Reads `path`; the defaults if it's missing or unreadable.
    /// The style is checked, since the file can be edited by hand.
    pub fn load(path: &Path) -> Self {
        let saved: Self = std::fs::read_to_string(path)
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default();
        Self {
            style: saved.style.clone().checked(),
            ..saved
        }
    }

    /// Writes to `path`.
    ///
    /// # Errors
    ///
    /// The I/O error if it can't be written.
    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        let text = serde_json::to_string_pretty(self)
            .map_err(std::io::Error::other)?;
        std::fs::write(path, text)
    }
}

#[cfg(test)]
mod tests {
    use super::Saved;
    use crate::corner::Corner;
    use crate::place::Edge;
    use crate::style::Style;

    #[test]
    fn round_trips() {
        let path = std::env::temp_dir()
            .join(format!("wallflex-reader-{}.json", std::process::id()));
        let saved = Saved {
            pinned: true,
            side: Some(Edge::Left),
            last: Some("Games/Setup.md".to_owned()),
            panel: Some((600, 800)),
            pane: Some(400),
            spot: Some((-1200, 40)),
            zoom: 1.25,
            style: Style {
                theme: "sepia".to_owned(),
                ..Style::default()
            },
            corner: Corner::BottomRight,
            contents: true,
        };
        saved.save(&path).unwrap();
        let loaded = Saved::load(&path);
        let _ = std::fs::remove_file(&path);
        assert_eq!(loaded, saved);
    }

    #[test]
    fn falls_back_to_defaults() {
        let missing = std::path::Path::new("Z:\\nowhere\\reader.json");
        assert_eq!(Saved::load(missing), Saved::default());
        assert_eq!(
            serde_json::from_str::<Saved>("{\"pinned\":true}").unwrap(),
            Saved {
                pinned: true,
                ..Saved::default()
            }
        );
    }
}
