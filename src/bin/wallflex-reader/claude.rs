//! The reader's Claude page: claude.ai shown in a second browser view
//! beside the guides, with the Contents column listing Claude's own
//! sidebar sections.
//!
//! The page picks a [`Section`] by name; only these fixed addresses are
//! ever opened from a request, so the page can't send the view anywhere
//! else. Links inside the view stay in it only on Claude's own hosts
//! ([`is_claude_host`]); anything else opens in the default browser.
//!
//! The page also sends the reading style as a stylesheet and a light or
//! dark [`Mode`]; [`style_script`] puts them into claude.ai as the text
//! of a `<style>` (so nothing in it runs) and keeps Claude in that mode.

use serde::Deserialize;

/// Prefix of every address the view shows.
const SCHEME: &str = "https://";
/// Hosts that are Claude's: the site, and Anthropic's sign-in pages.
const HOSTS: [&str; 2] = ["claude.ai", "anthropic.com"];

/// The longest stylesheet the page may send, in bytes.
pub const MAX_CSS: usize = 8192;
/// Where a new chat starts.
const NEW_CHAT: &str = "https://claude.ai/new";
/// The `<style>` element's id in claude.ai's page.
const STYLE_ID: &str = "wallflex-style";

/// Claude's light or dark look, to suit the reading theme.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    /// Light (the light and sepia themes).
    Light,
    /// Dark (the dark and black themes).
    Dark,
}

impl Mode {
    /// Claude's name for it (its `<html data-mode>`).
    const fn name(self) -> &'static str {
        match self {
            Self::Light => "light",
            Self::Dark => "dark",
        }
    }
}

/// Claude's own layout density (`data-density`) kept in the view: the
/// tighter of its two, to suit the reader's small text.
const DENSITY: &str = "compact";

/// The script that styles claude.ai: adds (or replaces) the reader's
/// stylesheet and holds Claude's mode and [`DENSITY`] on every element
/// that sets them (its sidebar and other parts carry their own, from the
/// account's settings), which its own code would otherwise set back.
/// Parts Claude marks with the opposite of the account's mode get the
/// opposite of `mode`.
/// `None` if the stylesheet is too long.
pub fn style_script(css: &str, mode: Mode) -> Option<String> {
    if css.len() > MAX_CSS {
        return None;
    }
    // JSON strings are valid JavaScript string literals.
    let css = serde_json::to_string(css).ok()?;
    let mode = serde_json::to_string(mode.name()).ok()?;
    let id = serde_json::to_string(STYLE_ID).ok()?;
    let density = serde_json::to_string(DENSITY).ok()?;
    Some(format!(
        "(() => {{
  const root = document.documentElement;
  let sheet = document.getElementById({id});
  if (!sheet) {{
    sheet = document.createElement('style');
    sheet.id = {id};
  }}
  sheet.textContent = {css};
  (document.head || root).append(sheet);
  window.wallflexMode = {mode};
  const other = (mode) => (mode === 'dark' ? 'light' : 'dark');
  // Claude marks most parts with the account's mode, and a few (such as
  // the incognito frame's buttons) with the opposite, to stand out. Each
  // keeps that relation to the reader's mode. `wallflexWas` is the value
  // Claude last wrote, `wallflexWrote` ours.
  const hold = (node) => {{
    const data = node.dataset;
    if (!data) return;
    if (data.mode) {{
      if (data.mode !== node.wallflexWrote) node.wallflexWas = data.mode;
      const account = root.wallflexWas || node.wallflexWas;
      const want = node.wallflexWas === account
        ? window.wallflexMode
        : other(window.wallflexMode);
      node.wallflexWrote = want;
      if (data.mode !== want) data.mode = want;
    }}
    if (data.density && data.density !== {density}) data.density = {density};
  }};
  const holdAll = (node) => {{
    window.wallflexHold(node);
    if (node.querySelectorAll) {{
      node.querySelectorAll('[data-mode], [data-density]')
        .forEach(window.wallflexHold);
    }}
  }};
  window.wallflexHold = hold;
  window.wallflexHoldAll = holdAll;
  if (!window.wallflexWatch) {{
    window.wallflexWatch = new MutationObserver((records) => {{
      for (const record of records) {{
        if (record.type === 'attributes') {{
          window.wallflexHold(record.target);
        }} else {{
          record.addedNodes.forEach(window.wallflexHoldAll);
        }}
      }}
    }});
    window.wallflexWatch.observe(root, {{
      attributes: true,
      attributeFilter: ['data-mode', 'data-density'],
      childList: true,
      subtree: true,
    }});
  }}
  holdAll(root);
}})();"
    ))
}

/// A rounded rectangle cut out of the Claude view so the page's own
/// menu and panels show through it: left, top, right, bottom and the
/// corners' diameter (a circle when that's the whole width), in device
/// pixels.
pub type Hole = [i32; 5];
/// The most holes the page may ask for.
pub const MAX_HOLES: usize = 64;

/// `holes` (measured from the page's corner) moved to be measured from
/// the view's corner, `spot` being where the view is (left, top, right,
/// bottom); those that miss the view, or are empty, are left out.
pub fn local_holes(spot: [i32; 4], holes: &[Hole]) -> Vec<Hole> {
    let [left, top, right, bottom] = spot;
    holes
        .iter()
        .take(MAX_HOLES)
        .filter(|[l, t, r, b, _]| {
            l < r && t < b && *l < right && *r > left && *t < bottom && *b > top
        })
        .map(|&[l, t, r, b, round]| {
            [
                l.saturating_sub(left),
                t.saturating_sub(top),
                r.saturating_sub(left),
                b.saturating_sub(top),
                round.max(0),
            ]
        })
        .collect()
}

/// A part of Claude's sidebar.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Section {
    /// Start a new chat.
    New,
    /// The list of past chats.
    Chats,
    /// Projects.
    Projects,
    /// Artifacts.
    Artifacts,
    /// Wherever the reader was in Claude when it last closed.
    Last,
}

impl Section {
    /// The address that shows this section; `last` is the address the
    /// reader closed on, used for [`Self::Last`] if it's still one of
    /// Claude's (else a new chat).
    pub fn url(self, last: Option<&str>) -> &str {
        match self {
            Self::New => NEW_CHAT,
            Self::Chats => "https://claude.ai/recents",
            Self::Projects => "https://claude.ai/projects",
            Self::Artifacts => "https://claude.ai/artifacts",
            Self::Last => {
                last.filter(|url| is_claude_host(url)).unwrap_or(NEW_CHAT)
            }
        }
    }
}

/// Whether `uri` is an https address on one of Claude's hosts (or a
/// sub-domain of one).
pub fn is_claude_host(uri: &str) -> bool {
    let Some(rest) = uri
        .get(..SCHEME.len())
        .filter(|head| head.eq_ignore_ascii_case(SCHEME))
        .and_then(|_| uri.get(SCHEME.len()..))
    else {
        return false;
    };
    let end = rest.find(['/', '?', '#', ':']).unwrap_or(rest.len());
    let host = rest.get(..end).unwrap_or_default().to_ascii_lowercase();
    HOSTS.iter().any(|known| {
        host == *known
            || host
                .strip_suffix(known)
                .is_some_and(|front| front.ends_with('.'))
    })
}

#[cfg(test)]
mod tests {
    use super::{
        MAX_CSS, MAX_HOLES, Mode, NEW_CHAT, Section, is_claude_host,
        local_holes, style_script,
    };

    #[test]
    fn moves_holes_into_the_view() {
        let spot = [100, 50, 500, 450];
        let holes = [
            [90, 40, 130, 80, 40],
            [600, 60, 650, 90, 0],
            [200, 200, 200, 300, 0],
            [300, 300, 340, 340, -5],
        ];
        assert_eq!(
            local_holes(spot, &holes),
            vec![[-10, -10, 30, 30, 40], [200, 250, 240, 290, 0]]
        );
        let many = vec![[100, 50, 110, 60, 0]; MAX_HOLES + 5];
        assert_eq!(local_holes(spot, &many).len(), MAX_HOLES);
    }

    #[test]
    fn styles_as_text() {
        let script = style_script("a { b: \"</style>\" }", Mode::Dark)
            .expect("short enough");
        assert!(
            script.contains(r#"sheet.textContent = "a { b: \"</style>\" }";"#)
        );
        assert!(script.contains(r#"window.wallflexMode = "dark";"#));
        assert!(script.contains(r#"data.density = "compact";"#));
        let long = "x".repeat(MAX_CSS + 1);
        assert_eq!(style_script(&long, Mode::Light), None);
        let read = |text| serde_json::from_str::<Mode>(text).ok();
        assert_eq!(read("\"light\""), Some(Mode::Light));
        assert_eq!(read("\"sepia\""), None);
    }

    #[test]
    fn sections_have_fixed_addresses() {
        let read = |text| serde_json::from_str::<Section>(text).ok();
        assert_eq!(read("\"new\""), Some(Section::New));
        assert_eq!(read("\"artifacts\""), Some(Section::Artifacts));
        assert_eq!(read("\"settings\""), None);
        assert!(is_claude_host(Section::Chats.url(None)));
        assert!(is_claude_host(Section::Projects.url(None)));
        // The saved address only counts for `Last`, and only on Claude.
        let chat = "https://claude.ai/chat/1";
        assert_eq!(read("\"last\""), Some(Section::Last));
        assert_eq!(Section::Last.url(Some(chat)), chat);
        assert_eq!(Section::New.url(Some(chat)), NEW_CHAT);
        assert_eq!(Section::Last.url(Some("https://evil.com/")), NEW_CHAT);
        assert_eq!(Section::Last.url(None), NEW_CHAT);
    }

    #[test]
    fn knows_claudes_hosts() {
        assert!(is_claude_host("https://claude.ai/"));
        assert!(is_claude_host("HTTPS://claude.ai:443/chat/1?x=1"));
        assert!(is_claude_host("https://console.anthropic.com/login"));
        assert!(!is_claude_host("http://claude.ai/"));
        assert!(!is_claude_host("https://evilclaude.ai/"));
        assert!(!is_claude_host("https://claude.ai.evil.com/"));
        assert!(!is_claude_host("https://example.com/claude.ai"));
        assert!(!is_claude_host("claude.ai"));
    }
}
