//! What the page and the app say to each other (JSON web messages), and
//! which addresses the reader may open.
//!
//! The page asks: `{"type": "ready" | "list" | "close" | "folder"}`,
//! `{"type": "open", "path": NAME}`, `{"type": "pin", "side": "left" |
//! "right"}`, `{"type": "move", "at": [X, Y]}`, `{"type": "style",
//! "style": STYLE}` or `{"type": "corner", "corner": NAME}`. The app
//! answers with `list` (the guides, the last one shown, whether it's
//! pinned and to which side, the style and the menu dot's corner),
//! `show` (a guide: a reading page as `html`, a file to load as `url`,
//! or an `error`), `pinned` and `escape` (Esc was
//! pressed).

use std::path::Path;

use serde::Deserialize;
use serde_json::json;

use crate::corner::Corner;
use crate::guides::{self, Kind};
use crate::place::Edge;
use crate::saved::Saved;
use crate::style::Style;

/// Something the page asks for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Request {
    /// The page has loaded and wants the guides.
    Ready,
    /// List the guides again.
    List,
    /// Show this guide.
    Open(String),
    /// Dock against this side, or float again if already docked there.
    Pin(Edge),
    /// Hide the window.
    Close,
    /// Open the guides folder in Explorer.
    Folder,
    /// The menu dot is being dragged: move the floating window with the
    /// mouse. It was pressed at this point in the page, in device pixels.
    Move((i32, i32)),
    /// Use and remember this reading style (not yet checked).
    Style(Style),
    /// Remember that the menu dot sits in this corner.
    Corner(Corner),
}

/// A request as the page sends it.
#[derive(Deserialize)]
struct RawRequest {
    /// What it asks for.
    #[serde(rename = "type")]
    kind: String,
    /// The guide, for `open`.
    path: Option<String>,
    /// The reading style, for `style`.
    style: Option<Style>,
    /// The side, for `pin`.
    side: Option<Edge>,
    /// The menu dot's corner, for `corner`.
    corner: Option<Corner>,
    /// Where the page was pressed, for `move`.
    at: Option<(i32, i32)>,
}

/// Reads a request from the page's JSON; `None` if it isn't one.
pub fn parse_request(json: &str) -> Option<Request> {
    let raw: RawRequest = serde_json::from_str(json).ok()?;
    Some(match raw.kind.as_str() {
        "ready" => Request::Ready,
        "list" => Request::List,
        "open" => Request::Open(raw.path?),
        "pin" => Request::Pin(raw.side?),
        "close" => Request::Close,
        "folder" => Request::Folder,
        "move" => Request::Move(raw.at?),
        "style" => Request::Style(raw.style?),
        "corner" => Request::Corner(raw.corner?),
        _ => return None,
    })
}

/// The `list` answer: every guide in `folder`, the one to show first
/// (the last one shown if it's still there, else the first), whether
/// the reader is pinned (and to which side), the reading style and the
/// corner the menu dot sits in.
pub fn list(folder: &Path, saved: &Saved) -> String {
    let names = guides::list(folder);
    let start = saved
        .last
        .as_deref()
        .filter(|last| names.iter().any(|n| n == last))
        .or(names.first().map(String::as_str));
    let entries: Vec<_> = names
        .iter()
        .map(|name| {
            json!({
                "path": name,
                "title": guides::title(name),
                "folder": guides::folder_of(name),
                "kind": guides::kind(name).map(Kind::name),
            })
        })
        .collect();
    json!({
        "type": "list",
        "guides": entries,
        "start": start,
        "pinned": saved.pinned,
        "side": saved.side,
        "style": saved.style,
        "corner": saved.corner,
        "where": folder.display().to_string(),
    })
    .to_string()
}

/// The `show` answer for guide `name` in `folder`, and whether it worked
/// (so the app can remember it).
pub fn show(folder: &Path, name: &str) -> (String, bool) {
    let title = guides::title(name);
    let answer = |field: &str, value: String| {
        let mut message = json!({
            "type": "show",
            "path": name,
            "title": title,
        });
        if let Some(object) = message.as_object_mut() {
            object.insert(field.to_owned(), value.into());
        }
        message.to_string()
    };
    let Some((path, kind)) = guides::resolve(folder, name)
        .and_then(|path| Some((path, guides::kind(name)?)))
    else {
        return (answer("error", "That isn't a guide.".to_owned()), false);
    };
    if !kind.is_rendered() {
        if !path.is_file() {
            return (answer("error", missing(name)), false);
        }
        return (answer("url", guides::url(name)), true);
    }
    match read_text(&path) {
        Ok(text) => (answer("html", guides::render(kind, name, &text)), true),
        Err(error) => (answer("error", error), false),
    }
}

/// Tells the page Esc was pressed (it closes its guide list, or asks to
/// close the window).
pub fn escape() -> String {
    json!({ "type": "escape" }).to_string()
}

/// The `pinned` answer: whether it's docked, and the side it docks to.
pub fn pinned(pinned: bool, side: Option<Edge>) -> String {
    json!({ "type": "pinned", "pinned": pinned, "side": side }).to_string()
}

/// Says a guide has gone.
fn missing(name: &str) -> String {
    format!("{name} isn't in the Guides folder any more.")
}

/// A Markdown or text file's contents (UTF-8, without a byte order
/// mark; bad bytes become U+FFFD), or why it can't be shown.
fn read_text(path: &Path) -> Result<String, String> {
    let shown = path.display();
    let size = std::fs::metadata(path)
        .map_err(|_| format!("{shown} isn't there any more."))?
        .len();
    if size > guides::MAX_TEXT_BYTES {
        return Err(format!("{shown} is too big to show (over 4 MB)."));
    }
    let bytes = std::fs::read(path)
        .map_err(|error| format!("Can't read {shown}: {error}"))?;
    let text = String::from_utf8_lossy(&bytes);
    Ok(text.strip_prefix('\u{feff}').unwrap_or(&text).to_owned())
}

/// What to do with an address the reader is about to open.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Navigation {
    /// Let it load.
    Allow,
    /// Stop it and show this guide instead.
    Guide(String),
    /// Stop it and open it in the default browser (or mail app).
    External(String),
    /// Stop it.
    Block,
}

/// Decides about `uri`, about to open in the whole page (`top`) or in
/// the guide's frame. The page itself is the app's (`about:`/`data:`);
/// Markdown and text guides are shown through the app; HTML, PDF and
/// other files from the guides folder load in the frame; web and mail
/// links go to their usual apps.
pub fn judge(uri: &str, top: bool) -> Navigation {
    let lower = uri.to_ascii_lowercase();
    if lower.starts_with("about:") || (top && lower.starts_with("data:")) {
        return Navigation::Allow;
    }
    if let Some(name) = guides::name_from_url(uri) {
        return match guides::kind(&name) {
            Some(kind) if top || kind.is_rendered() => Navigation::Guide(name),
            None if top => Navigation::Block,
            // HTML, PDF and their pictures load in the frame.
            Some(_) | None => Navigation::Allow,
        };
    }
    let external = ["https:", "http:", "mailto:"]
        .iter()
        .any(|scheme| lower.starts_with(scheme));
    if external {
        Navigation::External(uri.to_owned())
    } else {
        Navigation::Block
    }
}

#[cfg(test)]
mod tests {
    use super::{Navigation, Request, judge, parse_request, show};
    use crate::corner::Corner;
    use crate::place::Edge;

    #[test]
    fn reads_requests() {
        assert_eq!(parse_request(r#"{"type":"ready"}"#), Some(Request::Ready));
        assert_eq!(
            parse_request(r#"{"type":"open","path":"a.md"}"#),
            Some(Request::Open("a.md".to_owned()))
        );
        assert_eq!(parse_request(r#"{"type":"open"}"#), None);
        assert_eq!(
            parse_request(r#"{"type":"pin","side":"right"}"#),
            Some(Request::Pin(Edge::Right))
        );
        assert_eq!(parse_request(r#"{"type":"pin"}"#), None);
        assert_eq!(parse_request(r#"{"type":"pin","side":"up"}"#), None);
        let style = r#"{"type":"style","style":{"theme":"dark","size":18}}"#;
        let Some(Request::Style(style)) = parse_request(style) else {
            panic!("not a style");
        };
        assert_eq!((style.theme.as_str(), style.size), ("dark", 18));
        assert_eq!(style.font, "segoe");
        assert_eq!(
            parse_request(r#"{"type":"move","at":[14,-3]}"#),
            Some(Request::Move((14, -3)))
        );
        assert_eq!(parse_request(r#"{"type":"move"}"#), None);
        assert_eq!(
            parse_request(r#"{"type":"corner","corner":"bottom-left"}"#),
            Some(Request::Corner(Corner::BottomLeft))
        );
        assert_eq!(parse_request(r#"{"type":"corner","corner":"mid"}"#), None);
        assert_eq!(parse_request(r#"{"type":"corner"}"#), None);
        assert_eq!(parse_request(r#"{"type":"rm"}"#), None);
        assert_eq!(parse_request("nope"), None);
    }

    #[test]
    fn judges_addresses() {
        let guide = |n: &str| Navigation::Guide(n.to_owned());
        assert_eq!(judge("about:srcdoc", false), Navigation::Allow);
        assert_eq!(judge("data:text/html,x", true), Navigation::Allow);
        assert_eq!(judge("data:text/html,x", false), Navigation::Block);
        assert_eq!(
            judge("https://guides.wallflex/a%20b.md", false),
            guide("a b.md")
        );
        assert_eq!(
            judge("https://guides.wallflex/x.pdf", false),
            Navigation::Allow
        );
        assert_eq!(
            judge("https://guides.wallflex/x.pdf", true),
            guide("x.pdf")
        );
        assert_eq!(
            judge("https://guides.wallflex/p.png", false),
            Navigation::Allow
        );
        assert_eq!(
            judge("https://guides.wallflex/p.png", true),
            Navigation::Block
        );
        assert_eq!(
            judge("https://example.com/", false),
            Navigation::External("https://example.com/".to_owned())
        );
        assert_eq!(judge("file:///C:/x", false), Navigation::Block);
        assert_eq!(judge("javascript:alert(1)", true), Navigation::Block);
    }

    #[test]
    fn refuses_names_outside_the_folder() {
        let (answer, ok) = show(std::path::Path::new(r"C:\g"), "../x.md");
        assert!(!ok);
        assert!(answer.contains("\"error\""));
    }
}
