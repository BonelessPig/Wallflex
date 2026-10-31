//! The guides: files in the `Guides` folder next to Wallflex's settings.
//!
//! Guides are named by their path inside the folder, with `/` between
//! parts (`Games/Setup.md`). Every name the page sends is checked here
//! before it touches the disk, so it can't reach outside the folder.
//! HTML and PDF guides load straight from the folder through `WebView2`'s
//! virtual host ([`HOST`]); Markdown, text and LaTeX source are turned
//! into a reading page here, with one stylesheet, and shown without
//! scripts.

use std::borrow::Cow;
use std::collections::HashMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use pulldown_cmark::{
    CodeBlockKind, CowStr, Event, Options, Parser, Tag, TagEnd,
};

/// The guides folder, next to the settings file.
pub const FOLDER: &str = "Guides";
/// The virtual host the guides folder is served on.
pub const HOST: &str = "guides.wallflex";
/// Where guide URLs start.
const BASE_URL: &str = "https://guides.wallflex/";
/// How many folders deep a guide may sit.
const MAX_DEPTH: usize = 4;
/// The most guides listed.
const MAX_GUIDES: usize = 1_000;
/// The biggest Markdown or text guide shown, in megabytes. Converted
/// books run to tens of megabytes.
pub const MAX_TEXT_MB: u64 = 32;
/// The biggest Markdown or text guide shown, in bytes.
pub const MAX_TEXT_BYTES: u64 = MAX_TEXT_MB * 1024 * 1024;
/// The longest file or folder name accepted, in characters.
const MAX_PART_CHARS: usize = 120;
/// Characters Windows doesn't allow in a file name (besides control
/// characters).
pub const FORBIDDEN: [char; 9] = ['<', '>', ':', '"', '/', '\\', '|', '?', '*'];
/// Names Windows keeps for devices, with or without an extension.
const RESERVED: [&str; 22] = [
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6",
    "COM7", "COM8", "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6",
    "LPT7", "LPT8", "LPT9",
];
/// The reading pages' stylesheet.
const READING_CSS: &str = include_str!("reading.css");
/// The reading pages' content security policy: no scripts, pictures
/// and media only from the guides folder.
const READING_CSP: &str = "default-src 'none'; style-src 'unsafe-inline'; \
                           img-src https: data:; \
                           media-src https://guides.wallflex";

/// A web reading page's content security policy: no scripts, pictures
/// from secure addresses only.
const WEB_CSP: &str = concat!(
    "default-src 'none'; style-src 'unsafe-inline'; ",
    "img-src https: data:"
);

/// What kind of file a guide is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// Markdown, turned into a reading page.
    Markdown,
    /// Plain text, shown in a reading page.
    Text,
    /// A web page, shown as it is.
    Html,
    /// A PDF, in Edge's viewer.
    Pdf,
    /// LaTeX source, shown as text (it can be converted to Markdown).
    Tex,
}

/// Guide files, by extension (lower case).
const KINDS: [(&str, Kind); 8] = [
    ("md", Kind::Markdown),
    ("markdown", Kind::Markdown),
    ("txt", Kind::Text),
    ("html", Kind::Html),
    ("htm", Kind::Html),
    ("pdf", Kind::Pdf),
    ("tex", Kind::Tex),
    ("latex", Kind::Tex),
];

impl Kind {
    /// Whether the reader makes the page itself (else it loads the file).
    pub fn is_rendered(self) -> bool {
        matches!(self, Self::Markdown | Self::Text | Self::Tex)
    }

    /// Its name for the page.
    pub fn name(self) -> &'static str {
        match self {
            Self::Markdown => "markdown",
            Self::Text => "text",
            Self::Html => "html",
            Self::Pdf => "pdf",
            Self::Tex => "tex",
        }
    }
}

/// The kind of guide a file name is, by its extension; `None` for any
/// other file.
pub fn kind(name: &str) -> Option<Kind> {
    let (_, ext) = name.rsplit_once('.')?;
    let ext = ext.to_ascii_lowercase();
    KINDS
        .iter()
        .find(|(known, _)| *known == ext)
        .map(|&(_, kind)| kind)
}

/// Whether `part` is a plain file or folder name Windows accepts.
fn plain_part(part: &str) -> bool {
    let stem = part.split('.').next().unwrap_or_default();
    !part.is_empty()
        && part != "."
        && part != ".."
        && part.chars().count() <= MAX_PART_CHARS
        && !part.ends_with(['.', ' '])
        && !part
            .chars()
            .any(|c| c.is_control() || FORBIDDEN.contains(&c))
        && !RESERVED.iter().any(|r| r.eq_ignore_ascii_case(stem.trim()))
}

/// The file a guide name (`Games/Setup.md`) stands for inside `folder`:
/// `None` unless every part is a plain name, it's at most
/// [`MAX_DEPTH`] folders deep and it's a guide file.
pub fn resolve(folder: &Path, name: &str) -> Option<PathBuf> {
    let parts: Vec<&str> = name.split(['/', '\\']).collect();
    let fits = (1..=MAX_DEPTH.saturating_add(1)).contains(&parts.len());
    if !fits || !parts.iter().all(|p| plain_part(p)) {
        return None;
    }
    kind(name)?;
    Some(parts.iter().fold(folder.to_path_buf(), |p, n| p.join(n)))
}

/// Every guide in `folder` (up to [`MAX_GUIDES`]), by name: the ones at
/// the top first, then each folder's, in alphabetical order.
pub fn list(folder: &Path) -> Vec<String> {
    let mut found: Vec<Vec<String>> = Vec::new();
    // Folders still to look in, with their names inside `folder`. A
    // list rather than recursion, so the depth is plainly bounded.
    let mut pending = vec![(folder.to_path_buf(), Vec::<String>::new())];
    while let Some((dir, parts)) = pending.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let Ok(name) = entry.file_name().into_string() else {
                continue;
            };
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            if !plain_part(&name) || name.starts_with('.') {
                continue;
            }
            let is_guide = kind(&name).is_some();
            let mut path = parts.clone();
            path.push(name);
            if file_type.is_dir() && path.len() <= MAX_DEPTH {
                pending.push((entry.path(), path));
            } else if file_type.is_file() && is_guide {
                found.push(path);
            }
        }
        if found.len() >= MAX_GUIDES {
            found.truncate(MAX_GUIDES);
            break;
        }
    }
    found.sort_by_cached_key(|parts| {
        let (file, folders) = parts.split_last().unzip();
        (
            folders.map(|f| f.join("/").to_lowercase()),
            file.map(|f| f.to_lowercase()),
        )
    });
    found.into_iter().map(|parts| parts.join("/")).collect()
}

/// The folder part of a guide name (`Games` for `Games/Setup.md`), or
/// `""` at the top.
pub fn folder_of(name: &str) -> &str {
    name.rsplit_once('/').map_or("", |(folder, _)| folder)
}

/// A guide's title: its file name without the extension.
pub fn title(name: &str) -> &str {
    let file = name.rsplit_once('/').map_or(name, |(_, file)| file);
    file.rsplit_once('.').map_or(file, |(stem, _)| stem)
}

/// Whether `stem` plus `.md` makes a name [`resolve`] accepts.
pub fn resolve_stem(stem: &str) -> bool {
    resolve(Path::new(""), &format!("{stem}.md")).is_some()
}

/// A guide's address on the virtual host.
pub fn url(name: &str) -> String {
    let parts: Vec<String> = name.split('/').map(percent_encode).collect();
    format!("{BASE_URL}{}", parts.join("/"))
}

/// The guide name a virtual-host address stands for (the inverse of
/// [`url`]), or `None` for any other address. The name is not checked.
pub fn name_from_url(url: &str) -> Option<String> {
    let path = url.strip_prefix(BASE_URL)?;
    let path = path.split(['?', '#']).next().unwrap_or_default();
    let parts = path
        .split('/')
        .map(percent_decode)
        .collect::<Option<Vec<_>>>()?;
    Some(parts.join("/"))
}

/// `part` with everything but unreserved URL characters percent-encoded.
fn percent_encode(part: &str) -> String {
    let mut out = String::with_capacity(part.len());
    for byte in part.bytes() {
        let unreserved = byte.is_ascii_alphanumeric()
            || matches!(byte, b'-' | b'.' | b'_' | b'~');
        if unreserved {
            out.push(char::from(byte));
        } else {
            // Writing to a String can't fail.
            let _ = write!(out, "%{byte:02X}");
        }
    }
    out
}

/// `part` with `%XX` escapes decoded; `None` if an escape is broken or
/// the result isn't UTF-8.
pub fn percent_decode(part: &str) -> Option<String> {
    let mut bytes = Vec::with_capacity(part.len());
    let mut rest = part.as_bytes();
    while let Some((&byte, tail)) = rest.split_first() {
        if byte == b'%' {
            let hex = std::str::from_utf8(tail.get(..2)?).ok()?;
            bytes.push(u8::from_str_radix(hex, 16).ok()?);
            rest = tail.get(2..)?;
        } else {
            bytes.push(byte);
            rest = tail;
        }
    }
    String::from_utf8(bytes).ok()
}

/// `text` with the characters HTML gives meaning to escaped.
pub fn escape_html(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            other => out.push(other),
        }
    }
    out
}

/// A reading page for guide `name` of `kind` with contents `text`.
/// Links and pictures are made relative to the guide's folder; raw HTML
/// in Markdown is shown as text.
pub fn render(kind: Kind, name: &str, text: &str) -> String {
    let (class, body) = match kind {
        Kind::Markdown => ("markdown", markdown(text, &folder_url(name))),
        Kind::Text | Kind::Tex | Kind::Html | Kind::Pdf => {
            ("text", format!("<pre>{}</pre>", escape_html(text)))
        }
    };
    format!(
        "<!doctype html><html><head><meta charset=\"utf-8\">\
         <meta http-equiv=\"Content-Security-Policy\" \
         content=\"{READING_CSP}\"><title>{}</title><style>{READING_CSS}\
         </style></head><body class=\"{class}\"><main>{body}</main>\
         </body></html>",
        escape_html(title(name))
    )
}

/// The reading page for a web page, with `<!--TITLE-->` and
/// `<!--CONTENT-->` where the reader page puts the title (escaped) and
/// the cleaned article. Like the guides' pages it has no scripts, but
/// pictures may come from any secure address.
pub fn web_template() -> String {
    format!(
        concat!(
            "<!doctype html><html><head><meta charset=\"utf-8\">",
            "<meta http-equiv=\"Content-Security-Policy\" ",
            "content=\"{csp}\">",
            "<meta name=\"referrer\" content=\"no-referrer\">",
            "<title><!--TITLE--></title><style>{css}</style></head>",
            "<body class=\"markdown web\"><main><!--CONTENT--></main>",
            "</body></html>"
        ),
        csp = WEB_CSP,
        css = READING_CSS
    )
}

/// The address of the folder a guide is in, ending in `/`.
fn folder_url(name: &str) -> String {
    let folder = folder_of(name);
    if folder.is_empty() {
        BASE_URL.to_owned()
    } else {
        format!("{}/", url(folder))
    }
}

/// Markdown to HTML, with links and pictures based at `base` and an id
/// on every heading, so `#heading` links work.
fn markdown(text: &str, base: &str) -> String {
    let options = Options::ENABLE_TABLES
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_TASKLISTS
        | Options::ENABLE_FOOTNOTES
        | Options::ENABLE_MATH;
    let text = wrap_bare_environments(text);
    let events: Vec<Event<'_>> = Parser::new_ext(&text, options)
        .map(|event| match event {
            Event::Html(html) | Event::InlineHtml(html) => Event::Text(html),
            Event::InlineMath(tex) => math_event(unquote(&tex), false),
            Event::DisplayMath(tex) => math_event(unquote(&tex), true),
            Event::Start(Tag::Link {
                link_type,
                dest_url,
                title,
                id,
            }) => Event::Start(Tag::Link {
                link_type,
                dest_url: rebase(dest_url, base),
                title,
                id,
            }),
            Event::Start(Tag::Image {
                link_type,
                dest_url,
                title,
                id,
            }) => Event::Start(Tag::Image {
                link_type,
                dest_url: rebase(dest_url, base),
                title,
                id,
            }),
            other => other,
        })
        .collect();
    let events = fold_math_fences(events);
    let mut ids = heading_ids(&events).into_iter();
    let events = events.into_iter().map(|event| match event {
        Event::Start(Tag::Heading {
            level,
            id: None,
            classes,
            attrs,
        }) => Event::Start(Tag::Heading {
            level,
            id: ids.next().map(CowStr::from),
            classes,
            attrs,
        }),
        other => other,
    });
    let mut html = String::new();
    pulldown_cmark::html::push_html(&mut html, events);
    html
}

/// What the maths converter writes into its output for a command it
/// doesn't know.
const PARSE_ERROR: &str = "[PARSE ERROR";

/// Style commands the converter lacks, and the ones it has that look
/// closest.
const COMMAND_ALIASES: [(&str, &str); 6] = [
    (r"\mathcal", r"\mathscr"),
    (r"\mathbbm", r"\mathbb"),
    (r"\mathsf", r"\mathrm"),
    (r"\mathtt", r"\mathrm"),
    (r"\bm", r"\boldsymbol"),
    (r"\textbf", r"\mathbf"),
];

/// `tex` with the commands in `COMMAND_ALIASES` swapped for ones the
/// converter knows. Only whole commands change: `\mathcalx` stays.
fn alias_commands(tex: &str) -> String {
    let mut out = tex.to_owned();
    for (from, to) in COMMAND_ALIASES {
        let mut result = String::with_capacity(out.len());
        let mut rest = out.as_str();
        while let Some(at) = rest.find(from) {
            let (head, tail) = rest.split_at(at);
            let after = &tail[from.len()..];
            result.push_str(head);
            let whole = !after.starts_with(|c: char| c.is_ascii_alphabetic());
            result.push_str(if whole { to } else { from });
            rest = after;
        }
        result.push_str(rest);
        out = result;
    }
    out
}

/// Environments the converter doesn't know, with nothing to draw of their
/// own: removed, leaving what they hold.
const WRAPPER_ENVIRONMENTS: [&str; 8] = [
    "equation",
    "equation*",
    "gather",
    "gather*",
    "multline",
    "multline*",
    "split",
    "displaymath",
];

/// Environments the converter lacks that are `align` in all but name.
const ALIGN_ENVIRONMENTS: [&str; 5] =
    ["align*", "aligned", "alignedat", "eqnarray", "eqnarray*"];

/// Commands that only matter on paper.
const IGNORED_COMMANDS: [&str; 3] = [r"\nonumber", r"\notag", r"\displaystyle"];

/// `tex` rewritten into what the converter reads: wrapper environments
/// and `\label{..}` dropped, `aligned` as `align`, `cases` as an `align`
/// behind a brace.
fn normalize_tex(tex: &str) -> String {
    let mut out = tex.to_owned();
    for name in WRAPPER_ENVIRONMENTS {
        out = out.replace(&format!(r"\begin{{{name}}}"), "");
        out = out.replace(&format!(r"\end{{{name}}}"), "");
    }
    for name in IGNORED_COMMANDS {
        out = out.replace(name, "");
    }
    out = drop_labels(&out);
    for name in ALIGN_ENVIRONMENTS {
        out = out.replace(&format!(r"\begin{{{name}}}"), r"\begin{align}");
        out = out.replace(&format!(r"\end{{{name}}}"), r"\end{align}");
    }
    out = out.replace(r"\begin{cases}", r"\left\{\begin{align}");
    out.replace(r"\end{cases}", r"\end{align}\right.")
}

/// `tex` without its `\label{..}` commands.
fn drop_labels(tex: &str) -> String {
    let mut out = String::with_capacity(tex.len());
    let mut rest = tex;
    while let Some(at) = rest.find(r"\label{") {
        out.push_str(&rest[..at]);
        let after = &rest[at..];
        rest = after.split_once('}').map_or("", |(_, tail)| tail);
    }
    out.push_str(rest);
    out
}

/// Markdown with each stretch of lines holding a LaTeX environment that
/// pandoc left outside `$` (an `equation`, or a formula ending in
/// `cases`) wrapped in `$$`, so it is drawn as maths. Fenced code is
/// left alone.
fn wrap_bare_environments(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut lines = text.lines();
    let mut fenced = false;
    while let Some(line) = lines.next() {
        let start = line.trim_start();
        if start.starts_with("```") || start.starts_with("~~~") {
            fenced = !fenced;
        }
        if fenced || !line.contains(r"\begin{") || line.contains('$') {
            out.push_str(line);
            out.push('\n');
            continue;
        }
        let mut formula = line.trim().to_owned();
        while !formula.contains(r"\end{") {
            let Some(next) = lines.next() else {
                break;
            };
            formula.push(' ');
            formula.push_str(next.trim());
        }
        out.push_str("\n$$");
        out.push_str(&formula);
        out.push_str("$$\n\n");
    }
    out
}

/// `tex` without the backticks pandoc wraps around it (`$`x`$`).
fn unquote(tex: &str) -> &str {
    let trimmed = tex.trim();
    trimmed
        .strip_prefix('`')
        .and_then(|rest| rest.strip_suffix('`'))
        .unwrap_or(trimmed)
}

/// Replaces each fenced block tagged `math` (pandoc's display maths)
/// with the formula it holds.
fn fold_math_fences(events: Vec<Event<'_>>) -> Vec<Event<'_>> {
    let mut out = Vec::with_capacity(events.len());
    let mut formula: Option<String> = None;
    for event in events {
        match event {
            Event::Start(Tag::CodeBlock(CodeBlockKind::Fenced(ref info)))
                if info.trim() == "math" =>
            {
                formula = Some(String::new());
            }
            Event::Text(ref text) if formula.is_some() => {
                if let Some(tex) = &mut formula {
                    tex.push_str(text);
                }
            }
            Event::End(TagEnd::CodeBlock) if formula.is_some() => {
                let tex = formula.take().unwrap_or_default();
                out.push(math_event(&tex, true));
            }
            other => out.push(other),
        }
    }
    out
}

/// A `$...$` or `$$...$$` formula as `MathML`, which `WebView2` draws itself
/// (so no script is needed). A formula that can't be converted stays as
/// its TeX source in a code span, escaped like any other text.
fn math_event(tex: &str, display: bool) -> Event<'static> {
    let style = if display {
        latex2mathml::DisplayStyle::Block
    } else {
        latex2mathml::DisplayStyle::Inline
    };
    let tex = normalize_tex(&alias_commands(tex));
    // The converter reports what it can't read inside its answer.
    if let Ok(mathml) = latex2mathml::latex_to_mathml(&tex, style)
        && !mathml.contains(PARSE_ERROR)
    {
        return Event::Html(CowStr::from(mathml));
    }
    let escaped = tex
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;");
    Event::Html(CowStr::from(format!("<code>{escaped}</code>")))
}

/// An id for each heading without one, in order: its text in lower
/// case, with dashes for spaces (`-2`, `-3`, ... on repeats).
fn heading_ids(events: &[Event<'_>]) -> Vec<String> {
    let mut ids = Vec::new();
    let mut seen: HashMap<String, u32> = HashMap::new();
    let mut heading: Option<String> = None;
    for event in events {
        match event {
            Event::Start(Tag::Heading { id: None, .. }) => {
                heading = Some(String::new());
            }
            Event::Text(text) | Event::Code(text) => {
                if let Some(heading) = &mut heading {
                    heading.push_str(text);
                }
            }
            Event::End(TagEnd::Heading(_)) => {
                if let Some(text) = heading.take() {
                    let slug = slug(&text);
                    let count = seen.entry(slug.clone()).or_insert(0);
                    *count = count.saturating_add(1);
                    ids.push(if *count == 1 {
                        slug
                    } else {
                        format!("{slug}-{count}")
                    });
                }
            }
            _ => {}
        }
    }
    ids
}

/// Heading text as an id: letters and digits in lower case, dashes for
/// spaces and dashes, everything else dropped.
fn slug(text: &str) -> String {
    let mut out = String::new();
    for c in text.trim().chars() {
        if c.is_alphanumeric() || c == '_' {
            out.extend(c.to_lowercase());
        } else if c == ' ' || c == '-' {
            out.push('-');
        }
    }
    out
}

/// A link or picture address based at `base` (the guide's folder): in-page
/// (`#...`) and full (`https:...`) addresses stay as they are, and
/// `/...` starts at the top of the guides folder.
fn rebase<'a>(url: CowStr<'a>, base: &str) -> CowStr<'a> {
    let has_scheme = url.split_once(':').is_some_and(|(scheme, _)| {
        !scheme.is_empty()
            && scheme
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || "+-.".contains(c))
    });
    if url.is_empty() || url.starts_with('#') || has_scheme {
        return url;
    }
    let joined: Cow<'_, str> = if let Some(rest) = url.strip_prefix("//") {
        format!("https://{rest}").into()
    } else if let Some(rest) = url.strip_prefix('/') {
        format!("{BASE_URL}{rest}").into()
    } else {
        format!("{base}{url}").into()
    };
    CowStr::from(joined.into_owned())
}

#[cfg(test)]
mod tests {
    #[test]
    fn math_becomes_mathml() {
        let html = super::markdown(
            "Let $x^2$ be.

$$a+b$$
",
            "",
        );
        assert!(html.contains("<math"), "{html}");
        assert!(!html.contains("$x^2$"), "{html}");
    }

    #[test]
    fn unknown_commands_fall_back_or_alias() {
        let html = super::markdown(r"$\mathcal{L}=1$ $\nosuchthing$", "");
        assert!(!html.contains("PARSE ERROR"), "{html}");
        assert_eq!(html.matches("<math").count(), 1, "{html}");
        assert!(html.contains("<code>"), "{html}");
    }

    #[test]
    fn bare_environments_become_maths() {
        let md = r"\begin{equation} \text{A}(Q) = \frac{Q}{2} \end{equation}
where

M_{ij} = \begin{cases} 0 & \text{if } i \geq j \\ -\infty & i < j \end{cases}
After.
";
        let html = super::markdown(md, "");
        assert_eq!(html.matches("<math").count(), 2, "{html}");
        assert!(!html.contains("PARSE ERROR"), "{html}");
        assert!(!html.contains("begin"), "{html}");
    }

    #[test]
    fn attention_formula_converts() {
        let md = r"\begin{equation} \text{MaskedAttn}(Q, K, V) = \text{softmax}\!\left(\frac{QK^T}{\sqrt{d_k}} + M\right) V \end{equation}
";
        let html = super::markdown(md, "");
        assert_eq!(html.matches("<math").count(), 1, "{html}");
        assert!(!html.contains("PARSE ERROR"), "{html}");
        assert!(!html.contains("<code>"), "{html}");
    }

    #[test]
    fn pandoc_maths_loses_its_backticks() {
        let md = "Let $`x^2`$ be.

``` math
a+b
```
";
        let html = super::markdown(md, "");
        assert!(!html.contains('`'), "{html}");
        assert!(!html.contains("<code"), "{html}");
        assert_eq!(html.matches("<math").count(), 2, "{html}");
    }

    use super::{
        Kind, folder_of, kind, list, name_from_url, render, resolve, slug,
        title, url,
    };
    use std::path::Path;

    #[test]
    fn knows_guide_files() {
        assert_eq!(kind("a.MD"), Some(Kind::Markdown));
        assert_eq!(kind("dir/a.htm"), Some(Kind::Html));
        assert_eq!(kind("a.pdf"), Some(Kind::Pdf));
        assert_eq!(kind("a.txt"), Some(Kind::Text));
        assert_eq!(kind("a.exe"), None);
        assert_eq!(kind("md"), None);
        assert!(Kind::Markdown.is_rendered() && !Kind::Pdf.is_rendered());
    }

    #[test]
    fn resolves_only_plain_names() {
        let root = Path::new(r"C:\g");
        assert_eq!(
            resolve(root, "Games/Setup.md"),
            Some(root.join("Games").join("Setup.md"))
        );
        assert!(resolve(root, r"a\b.txt").is_some());
        for bad in [
            "../x.md",
            "a/../../x.md",
            "C:/x.md",
            "/x.md",
            "a//x.md",
            "x.exe",
            "con.md",
            "a/b/c/d/e/x.md",
            "x.md.",
            "",
        ] {
            assert_eq!(resolve(root, bad), None, "{bad}");
        }
        assert!(resolve(root, "a/b/c/d/x.md").is_some());
    }

    #[test]
    fn names_parts() {
        assert_eq!(folder_of("Games/Setup.md"), "Games");
        assert_eq!(folder_of("Setup.md"), "");
        assert_eq!(title("Games/Set.up.md"), "Set.up");
        assert_eq!(title("notes"), "notes");
    }

    #[test]
    fn makes_and_reads_urls() {
        let name = "My games/Set up #1.md";
        let address = url(name);
        assert_eq!(
            address,
            "https://guides.wallflex/My%20games/Set%20up%20%231.md"
        );
        assert_eq!(name_from_url(&address).as_deref(), Some(name));
        assert_eq!(
            name_from_url("https://guides.wallflex/a.md#top").as_deref(),
            Some("a.md")
        );
        assert_eq!(name_from_url("https://example.com/a.md"), None);
        assert_eq!(name_from_url("https://guides.wallflex/%zz.md"), None);
        assert_eq!(name_from_url("https://guides.wallflex/%ff.md"), None);
    }

    #[test]
    fn escapes_raw_html_in_markdown() {
        let page = render(
            Kind::Markdown,
            "a.md",
            "# Hi\n\n<script>alert(1)</script>\n\nok <b>x</b>",
        );
        assert!(!page.contains("<script>alert"));
        assert!(page.contains("&lt;script&gt;"));
        assert!(page.contains("&lt;b&gt;"));
        assert!(page.contains("Content-Security-Policy"));
    }

    #[test]
    fn bases_links_on_the_guide_folder() {
        let page = render(
            Kind::Markdown,
            "Games/a.md",
            "[next](b.md) [top](/c.md) [web](https://x.org) [here](#hi) \
             ![pic](img/p.png)",
        );
        assert!(page.contains("href=\"https://guides.wallflex/Games/b.md\""));
        assert!(page.contains("href=\"https://guides.wallflex/c.md\""));
        assert!(page.contains("href=\"https://x.org\""));
        assert!(page.contains("href=\"#hi\""));
        assert!(
            page.contains("src=\"https://guides.wallflex/Games/img/p.png\"")
        );
    }

    #[test]
    fn gives_headings_ids() {
        let page = render(
            Kind::Markdown,
            "a.md",
            "# Getting started\n\n## Step 1\n\n## Step 1",
        );
        assert!(page.contains("<h1 id=\"getting-started\">"));
        assert!(page.contains("<h2 id=\"step-1\">"));
        assert!(page.contains("<h2 id=\"step-1-2\">"));
        assert_eq!(slug(" A, b-C! "), "a-b-c");
    }

    #[test]
    fn shows_text_as_is() {
        let page = render(Kind::Text, "notes.txt", "a < b\n  indented");
        assert!(page.contains("<pre>a &lt; b\n  indented</pre>"));
        assert!(page.contains("<title>notes</title>"));
    }

    #[test]
    fn lists_guides_in_order() {
        let dir = std::env::temp_dir()
            .join(format!("wallflex-reader-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        for file in ["b.md", "A.txt", "z/x.pdf", "Y/w.html", "skip.exe"] {
            let path = dir.join(file);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, "x").unwrap();
        }
        std::fs::create_dir_all(dir.join("a/b/c/d/e")).unwrap();
        std::fs::write(dir.join("a/b/c/d/e/deep.md"), "x").unwrap();
        std::fs::write(dir.join("a/b/c/d/ok.md"), "x").unwrap();
        let names = list(&dir);
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(
            names,
            ["A.txt", "b.md", "a/b/c/d/ok.md", "Y/w.html", "z/x.pdf"]
        );
    }
}
