//! The corners of the reader's window the menu dot can sit in, chosen in
//! the page's **Aa** panel and remembered in `reader.json`.
//!
//! PDFs are drawn by Edge's own viewer, whose toolbar (contents, page,
//! zoom) runs along the top, so a bottom corner keeps the dot off it.

use serde::{Deserialize, Serialize};

/// A corner of the reader's window.
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize,
)]
#[serde(rename_all = "kebab-case")]
pub enum Corner {
    /// Top-left, where the dot has always been.
    #[default]
    TopLeft,
    /// Top-right.
    TopRight,
    /// Bottom-left.
    BottomLeft,
    /// Bottom-right.
    BottomRight,
}

#[cfg(test)]
mod tests {
    use super::Corner;

    #[test]
    fn uses_kebab_case_names() {
        let name = |corner| serde_json::to_string(&corner).unwrap();
        assert_eq!(name(Corner::TopLeft), "\"top-left\"");
        assert_eq!(name(Corner::BottomRight), "\"bottom-right\"");
        let read = |text| serde_json::from_str::<Corner>(text).ok();
        assert_eq!(read("\"bottom-left\""), Some(Corner::BottomLeft));
        assert_eq!(read("\"middle\""), None);
    }
}
