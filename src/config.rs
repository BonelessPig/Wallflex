//! Settings from `config.toml`: the background (a colour, transparent
//! or a picture) and which widgets go where on each monitor. The file
//! format is documented in `config.default.toml`, which is also written
//! for new users.

use std::fmt;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::backdrop::{Backdrop, Background, Fit, Wallpaper};
use crate::color::Rgb;
use crate::geometry::Rect;
use crate::pace::Mode;
use crate::widgets::{Parts, Widget};

/// The commented default config, written on first run.
pub const DEFAULT_CONFIG: &str = include_str!("../config.default.toml");

/// Seconds of history the network graph shows by default.
const DEFAULT_NET_SECONDS: u32 = crate::widgets::NET_DEFAULT_SECONDS;
/// Seconds of history the network graph can show.
const NET_SECONDS_RANGE: std::ops::RangeInclusive<u32> = 10..=300;

/// Default gap from the anchored edges, in design units.
const DEFAULT_OFFSET: f64 = 64.0;
/// Smallest and largest `size` accepted.
const SIZE_RANGE: (f64, f64) = (0.25, 4.0);
/// Smallest and largest `image_zoom` accepted.
const ZOOM_RANGE: (f64, f64) = (0.1, 10.0);

/// Validated settings.
#[derive(Clone, Debug, PartialEq)]
pub struct Settings {
    /// Background colour, or transparent; a layout can change it.
    pub background: Background,
    /// Picture shown on every monitor whose layout doesn't change it.
    pub image: Option<PathBuf>,
    /// How pictures are fitted, unless a layout says otherwise.
    pub image_fit: Fit,
    /// Layouts in file order; a monitor uses the first that matches.
    pub layouts: Vec<Layout>,
    /// Discord, if set up.
    pub discord: Option<DiscordSettings>,
    /// How smoothly to animate.
    pub performance: Mode,
    /// Your internet plan's download speed (Mbit/s), if set: the network
    /// graph's colours reach red there (or at the adapter's speed, if
    /// that's lower).
    pub net_max_mbps: Option<f64>,
    /// How many seconds of history the network graph shows.
    pub net_seconds: u32,
    /// Which parts the panel shows.
    pub parts: Parts,
}

/// The `[discord]` section: your Discord application's details.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiscordSettings {
    /// Application (client) id.
    pub client_id: String,
    /// Client secret.
    pub client_secret: String,
    /// Redirect URI registered for the application.
    pub redirect_uri: String,
    /// Bot token, for who's online and friends.
    pub bot_token: Option<String>,
    /// Friends' user ids, for the FRIENDS page.
    pub friends: Vec<String>,
}

/// What goes on the monitors a [`MonitorMatch`] selects.
#[derive(Clone, Debug, PartialEq)]
pub struct Layout {
    /// Which monitors this layout is for.
    pub monitor: MonitorMatch,
    /// Scale factor for everything in it.
    pub size: f64,
    /// The widgets, drawn in order.
    pub widgets: Vec<Placement>,
    /// Its own background, if it changes the top-level one.
    pub background: Option<Background>,
    /// Its own picture, or none, if it changes the top-level one.
    pub image: ImageChoice,
    /// How its picture is fitted, if it changes the top-level setting.
    pub image_fit: Option<Fit>,
    /// How much bigger its picture is drawn, if set (else automatic).
    pub image_zoom: Option<f64>,
    /// How far its picture is moved (right, down), in pixels.
    pub image_offset: (f64, f64),
}

/// A layout's `image`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum ImageChoice {
    /// Left out: the top-level picture, if any.
    #[default]
    Inherit,
    /// `image = ""`: no picture on these monitors.
    Off,
    /// This picture.
    Path(PathBuf),
}

/// One widget and where it goes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Placement {
    /// The widget.
    pub widget: Widget,
    /// The corner or edge it sits against.
    pub anchor: Anchor,
    /// How far in from that corner or edge, in design units.
    pub offset: (f64, f64),
}

/// Which monitors a layout applies to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MonitorMatch {
    /// Every monitor.
    Any,
    /// The primary monitor.
    Primary,
    /// Monitors with this resolution.
    Resolution(i32, i32),
    /// The monitor with this Windows name (`DISPLAY2`), any case.
    Name(String),
}

/// A monitor, as far as layout matching cares.
#[derive(Clone, Copy, Debug)]
pub struct MonitorInfo<'a> {
    /// Windows name, e.g. `\\.\DISPLAY1`.
    pub name: &'a str,
    /// Width in pixels.
    pub width: i32,
    /// Height in pixels.
    pub height: i32,
    /// Whether it's the primary monitor.
    pub primary: bool,
}

/// The corner, edge or centre a widget is placed against.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Anchor {
    /// Top-left corner.
    TopLeft,
    /// Middle of the top edge.
    Top,
    /// Top-right corner.
    TopRight,
    /// Middle of the left edge.
    Left,
    /// Centre.
    Center,
    /// Middle of the right edge.
    Right,
    /// Bottom-left corner.
    BottomLeft,
    /// Middle of the bottom edge.
    Bottom,
    /// Bottom-right corner.
    BottomRight,
}

/// Where along one axis a widget sits.
#[derive(Clone, Copy)]
enum Side {
    /// Against the start (left or top).
    Start,
    /// Centred.
    Middle,
    /// Against the end (right or bottom).
    End,
}

impl Anchor {
    /// Horizontal and vertical sides.
    fn sides(self) -> (Side, Side) {
        match self {
            Self::TopLeft => (Side::Start, Side::Start),
            Self::Top => (Side::Middle, Side::Start),
            Self::TopRight => (Side::End, Side::Start),
            Self::Left => (Side::Start, Side::Middle),
            Self::Center => (Side::Middle, Side::Middle),
            Self::Right => (Side::End, Side::Middle),
            Self::BottomLeft => (Side::Start, Side::End),
            Self::Bottom => (Side::Middle, Side::End),
            Self::BottomRight => (Side::End, Side::End),
        }
    }

    /// The offset used when x or y is left out: a gap from an edge, but
    /// none along a centred axis.
    fn default_offset(self) -> (f64, f64) {
        let gap = |side| match side {
            Side::Middle => 0.0,
            Side::Start | Side::End => DEFAULT_OFFSET,
        };
        let (horizontal, vertical) = self.sides();
        (gap(horizontal), gap(vertical))
    }

    /// Places a `size` (pixels) widget against this anchor of `area`,
    /// moved in by `offset` (pixels), then kept inside `bounds` so it
    /// never ends up off the monitor.
    pub fn place(
        self,
        area: Rect,
        size: (i32, i32),
        offset: (i32, i32),
        bounds: Rect,
    ) -> Rect {
        let (horizontal, vertical) = self.sides();
        let left = along(horizontal, (area.left, area.right), size.0, offset.0);
        let top = along(vertical, (area.top, area.bottom), size.1, offset.1);
        let left = keep_inside(left, size.0, (bounds.left, bounds.right));
        let top = keep_inside(top, size.1, (bounds.top, bounds.bottom));
        Rect {
            left,
            top,
            right: left.saturating_add(size.0),
            bottom: top.saturating_add(size.1),
        }
    }
}

/// The start position along one axis of `span` for a widget `length`
/// long, `offset` in from the chosen side.
fn along(
    side: Side,
    (start, end): (i32, i32),
    length: i32,
    offset: i32,
) -> i32 {
    match side {
        Side::Start => start.saturating_add(offset),
        Side::Middle => {
            let free = end.saturating_sub(start).saturating_sub(length);
            start.saturating_add(free / 2).saturating_add(offset)
        }
        Side::End => end.saturating_sub(offset).saturating_sub(length),
    }
}

/// Moves `position` so `length` fits inside `start..end`; if it can't
/// fit, it starts at `start`.
fn keep_inside(position: i32, length: i32, (start, end): (i32, i32)) -> i32 {
    let last = end.saturating_sub(length);
    position.min(last).max(start)
}

impl MonitorMatch {
    /// Whether `monitor` is one of the monitors this selects.
    fn matches(&self, monitor: &MonitorInfo<'_>) -> bool {
        match self {
            Self::Any => true,
            Self::Primary => monitor.primary,
            Self::Resolution(w, h) => {
                monitor.width == *w && monitor.height == *h
            }
            Self::Name(name) => {
                let short = monitor.name.trim_start_matches(r"\\.\");
                short.eq_ignore_ascii_case(name)
            }
        }
    }
}

impl Settings {
    /// The layout for `monitor`: the first one that matches.
    pub fn layout_for(&self, monitor: &MonitorInfo<'_>) -> Option<&Layout> {
        self.layouts.iter().find(|l| l.monitor.matches(monitor))
    }

    /// What goes behind the widgets on a monitor using `layout` (`None`
    /// for a monitor no layout matches): the layout's own background
    /// and picture where it gives them, else the top-level ones.
    pub fn backdrop_for(&self, layout: Option<&Layout>) -> Backdrop {
        let background =
            layout.and_then(|l| l.background).unwrap_or(self.background);
        let path = match layout.map(|l| &l.image) {
            None | Some(ImageChoice::Inherit) => self.image.clone(),
            Some(ImageChoice::Off) => None,
            Some(ImageChoice::Path(path)) => Some(path.clone()),
        };
        let fit = layout.and_then(|l| l.image_fit).unwrap_or(self.image_fit);
        Backdrop {
            background,
            image: path.map(|path| Wallpaper {
                path,
                fit,
                zoom: layout.and_then(|l| l.image_zoom),
                offset: layout.map_or((0.0, 0.0), |l| l.image_offset),
            }),
        }
    }

    /// The settings with relative picture paths made relative to `dir`
    /// (the config file's folder).
    #[must_use]
    pub fn with_base_dir(mut self, dir: &Path) -> Self {
        let resolve = |path: &mut PathBuf| {
            *path = crate::paths::native_separators(path);
            if path.is_relative() {
                *path = dir.join(&*path);
            }
        };
        if let Some(path) = &mut self.image {
            resolve(path);
        }
        for layout in &mut self.layouts {
            if let ImageChoice::Path(path) = &mut layout.image {
                resolve(path);
            }
        }
        self
    }
}

impl Default for Settings {
    /// The built-in defaults, the same as `config.default.toml`.
    fn default() -> Self {
        Self {
            background: Background::Color(Rgb::BLACK),
            image: None,
            image_fit: Fit::Fill,
            layouts: vec![Layout {
                monitor: MonitorMatch::Any,
                size: 1.0,
                widgets: vec![Placement {
                    widget: Widget::Panel,
                    anchor: Anchor::BottomRight,
                    offset: (DEFAULT_OFFSET, DEFAULT_OFFSET),
                }],
                background: None,
                image: ImageChoice::Inherit,
                image_fit: None,
                image_zoom: None,
                image_offset: (0.0, 0.0),
            }],
            discord: None,
            performance: Mode::Smooth,
            net_max_mbps: None,
            net_seconds: DEFAULT_NET_SECONDS,
            parts: Parts::default(),
        }
    }
}

/// Why a config file was rejected.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConfigError(pub String);

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ConfigError {}

/// The file as written, before validation.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawConfig {
    /// `"#RRGGBB"` or `"transparent"`.
    background: Option<String>,
    /// Picture file.
    image: Option<String>,
    /// `"fill"`, `"fit"`, `"stretch"` or `"center"`.
    image_fit: Option<String>,
    /// `[[layout]]` tables.
    #[serde(default)]
    layout: Vec<RawLayout>,
    /// `[discord]` table.
    discord: Option<RawDiscord>,
    /// `"smooth"` or `"light"`.
    performance: Option<String>,
    /// `[network]` table.
    network: Option<RawNetwork>,
    /// `[panel]` table.
    panel: Option<RawPanel>,
}

/// `[panel]` as written: which parts show (all if left out).
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawPanel {
    /// CPU columns.
    cpu: Option<bool>,
    /// Network graph.
    network: Option<bool>,
    /// RAM line.
    ram: Option<bool>,
    /// SSD temperatures.
    ssd: Option<bool>,
    /// GPU numbers to show, 1 = the first.
    gpus: Option<Vec<i64>>,
}

/// `[network]` as written.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawNetwork {
    /// Internet download speed, Mbit/s.
    max_mbps: Option<f64>,
    /// Seconds of history the graph shows.
    seconds: Option<i64>,
}

/// `[discord]` as written.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawDiscord {
    /// Application id.
    client_id: String,
    /// Client secret.
    client_secret: String,
    /// Redirect URI.
    redirect_uri: Option<String>,
    /// Bot token.
    bot_token: Option<String>,
    /// Friends' user ids.
    #[serde(default)]
    friends: Vec<String>,
}

/// One `[[layout]]` as written.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawLayout {
    /// Which monitors.
    monitor: Option<String>,
    /// Scale factor.
    size: Option<f64>,
    /// `"#RRGGBB"` or `"transparent"`.
    background: Option<String>,
    /// Picture file, or `""` for none.
    image: Option<String>,
    /// How the picture is fitted.
    image_fit: Option<String>,
    /// How much bigger the picture is drawn.
    image_zoom: Option<f64>,
    /// How far the picture is moved, `[x, y]` in pixels.
    image_offset: Option<Vec<f64>>,
    /// `[[layout.widget]]` tables.
    #[serde(default)]
    widget: Vec<RawWidget>,
}

/// One `[[layout.widget]]` as written.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawWidget {
    /// Widget name.
    kind: String,
    /// Anchor name.
    anchor: Option<String>,
    /// Horizontal offset.
    x: Option<f64>,
    /// Vertical offset.
    y: Option<f64>,
    /// GPU number for `kind = "gpu"`, from 1.
    gpu: Option<u32>,
}

/// Parses and validates a config file's text.
///
/// # Errors
///
/// A [`ConfigError`] saying what's wrong and where.
pub fn parse(text: &str) -> Result<Settings, ConfigError> {
    let raw: RawConfig =
        toml::from_str(text).map_err(|e| ConfigError(e.to_string()))?;
    let background = raw
        .background
        .as_deref()
        .map(parse_background)
        .transpose()
        .map_err(ConfigError)?
        .unwrap_or(Background::Color(Rgb::BLACK));
    let image = match raw.image.as_deref().map(str::trim) {
        None | Some("") => None,
        Some(path) => Some(PathBuf::from(path)),
    };
    let image_fit = raw
        .image_fit
        .as_deref()
        .map(parse_fit)
        .transpose()
        .map_err(ConfigError)?
        .unwrap_or_default();
    let layouts = (1_u32..)
        .zip(raw.layout)
        .map(|(number, layout)| {
            parse_layout(layout)
                .map_err(|e| ConfigError(format!("layout {number}: {e}")))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let discord = raw
        .discord
        .map(parse_discord)
        .transpose()
        .map_err(|e| ConfigError(format!("discord: {e}")))?;
    let performance = match raw.performance.as_deref() {
        None | Some("smooth") => Mode::Smooth,
        Some("light") => Mode::Light,
        Some(other) => {
            return Err(ConfigError(format!(
                "performance: '{other}' isn't \"smooth\" or \"light\""
            )));
        }
    };
    let parts = raw
        .panel
        .map(parse_panel)
        .transpose()
        .map_err(|e| ConfigError(format!("panel: {e}")))?
        .unwrap_or_default();
    let net_seconds =
        parse_net_seconds(raw.network.as_ref().and_then(|n| n.seconds))?;
    let net_max_mbps = match raw.network.and_then(|n| n.max_mbps) {
        Some(mbps) if mbps.is_finite() && mbps > 0.0 => Some(mbps),
        Some(mbps) => {
            return Err(ConfigError(format!(
                "network: max_mbps = {mbps} must be more than 0"
            )));
        }
        None => None,
    };
    Ok(Settings {
        background,
        image,
        image_fit,
        layouts,
        discord,
        performance,
        net_max_mbps,
        net_seconds,
        parts,
    })
}

/// Checks `[network] seconds`, if given.
fn parse_net_seconds(seconds: Option<i64>) -> Result<u32, ConfigError> {
    let Some(seconds) = seconds else {
        return Ok(DEFAULT_NET_SECONDS);
    };
    u32::try_from(seconds)
        .ok()
        .filter(|s| NET_SECONDS_RANGE.contains(s))
        .ok_or_else(|| {
            ConfigError(format!(
                "network: seconds = {seconds} must be from {} to {}",
                NET_SECONDS_RANGE.start(),
                NET_SECONDS_RANGE.end()
            ))
        })
}

/// Validates `[panel]`.
fn parse_panel(raw: RawPanel) -> Result<Parts, String> {
    let gpus = raw
        .gpus
        .map(|numbers| {
            numbers
                .into_iter()
                .map(|n| {
                    usize::try_from(n)
                        .ok()
                        .and_then(|n| n.checked_sub(1))
                        .ok_or_else(|| {
                            format!("gpus: {n} isn't a GPU number (1 or more)")
                        })
                })
                .collect::<Result<Vec<_>, _>>()
        })
        .transpose()?;
    Ok(Parts {
        cpu: raw.cpu.unwrap_or(true),
        network: raw.network.unwrap_or(true),
        ram: raw.ram.unwrap_or(true),
        ssd: raw.ssd.unwrap_or(true),
        gpus,
    })
}

/// Default redirect URI, as `DiscordBridge`'s setup guide registers.
const DEFAULT_REDIRECT: &str = "http://localhost";

/// Validates the `[discord]` section.
fn parse_discord(raw: RawDiscord) -> Result<DiscordSettings, String> {
    let is_id =
        |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    if !is_id(&raw.client_id) {
        return Err("client_id must be the application's number".into());
    }
    if raw.client_secret.trim().is_empty() {
        return Err("client_secret is empty".into());
    }
    if let Some(bad) = raw.friends.iter().find(|f| !is_id(f)) {
        return Err(format!("friends: '{bad}' isn't a user id"));
    }
    Ok(DiscordSettings {
        client_id: raw.client_id,
        client_secret: raw.client_secret,
        redirect_uri: raw
            .redirect_uri
            .unwrap_or_else(|| DEFAULT_REDIRECT.to_owned()),
        bot_token: raw.bot_token.filter(|t| !t.trim().is_empty()),
        friends: raw.friends,
    })
}

/// Validates one layout.
fn parse_layout(raw: RawLayout) -> Result<Layout, String> {
    let monitor = parse_monitor(raw.monitor.as_deref().unwrap_or("*"))?;
    let size = raw.size.unwrap_or(1.0);
    if !(SIZE_RANGE.0..=SIZE_RANGE.1).contains(&size) {
        return Err(format!(
            "size {size} is outside {} to {}",
            SIZE_RANGE.0, SIZE_RANGE.1
        ));
    }
    let widgets = (1_u32..)
        .zip(raw.widget)
        .map(|(number, widget)| {
            parse_widget(&widget).map_err(|e| format!("widget {number}: {e}"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let background = raw
        .background
        .as_deref()
        .map(parse_background)
        .transpose()?;
    let image = match raw.image.as_deref().map(str::trim) {
        None => ImageChoice::Inherit,
        Some("") => ImageChoice::Off,
        Some(path) => ImageChoice::Path(PathBuf::from(path)),
    };
    let image_fit = raw.image_fit.as_deref().map(parse_fit).transpose()?;
    let image_zoom = raw
        .image_zoom
        .map(|zoom| {
            let (low, high) = ZOOM_RANGE;
            if (low..=high).contains(&zoom) {
                Ok(zoom)
            } else {
                Err(format!("image_zoom {zoom} is outside {low} to {high}"))
            }
        })
        .transpose()?;
    let image_offset = match raw.image_offset.as_deref() {
        None => (0.0, 0.0),
        Some(&[x, y]) if x.is_finite() && y.is_finite() => (x, y),
        Some(_) => {
            return Err("image_offset must be [x, y], two numbers".into());
        }
    };
    Ok(Layout {
        monitor,
        size,
        widgets,
        background,
        image,
        image_fit,
        image_zoom,
        image_offset,
    })
}

/// Parses a `background` value.
fn parse_background(text: &str) -> Result<Background, String> {
    if text.trim().eq_ignore_ascii_case("transparent") {
        return Ok(Background::Transparent);
    }
    Rgb::from_hex(text).map(Background::Color).ok_or_else(|| {
        format!(
            "background: '{text}' isn't a #RRGGBB colour or \"transparent\""
        )
    })
}

/// Parses an `image_fit` value.
fn parse_fit(text: &str) -> Result<Fit, String> {
    Ok(match text {
        "fill" => Fit::Fill,
        "fit" => Fit::Contain,
        "stretch" => Fit::Stretch,
        "center" | "centre" => Fit::Center,
        "span" => Fit::Span,
        other => {
            return Err(format!(
                "image_fit: '{other}' isn't \"fill\", \"fit\", \"stretch\", \
                 \"center\" or \"span\""
            ));
        }
    })
}

/// Validates one widget.
fn parse_widget(raw: &RawWidget) -> Result<Placement, String> {
    let widget = match (raw.kind.as_str(), raw.gpu) {
        ("panel", None) => Widget::Panel,
        ("cpu", None) => Widget::Cpu,
        ("network", None) => Widget::Network,
        ("gpus", None) => Widget::Gpus,
        ("info", None) => Widget::Info,
        ("discord", None) => Widget::Discord,
        ("visualizer", None) => Widget::Visualizer,
        ("gpu", Some(number)) if number >= 1 => {
            let index = number.saturating_sub(1);
            Widget::Gpu(usize::try_from(index).map_err(|e| e.to_string())?)
        }
        ("gpu", _) => return Err("kind = \"gpu\" needs gpu = 1 or more".into()),
        (_, Some(_)) => {
            return Err("gpu = ... only goes with kind = \"gpu\"".into());
        }
        (other, None) => return Err(format!("unknown kind '{other}'")),
    };
    let anchor = parse_anchor(raw.anchor.as_deref().unwrap_or("bottom-right"))?;
    let (default_x, default_y) = anchor.default_offset();
    let x = raw.x.unwrap_or(default_x);
    let y = raw.y.unwrap_or(default_y);
    if !x.is_finite() || !y.is_finite() {
        return Err("x and y must be numbers".into());
    }
    Ok(Placement {
        widget,
        anchor,
        offset: (x, y),
    })
}

/// Parses a `monitor` value.
fn parse_monitor(text: &str) -> Result<MonitorMatch, String> {
    let text = text.trim();
    if text == "*" {
        return Ok(MonitorMatch::Any);
    }
    if text.eq_ignore_ascii_case("primary") {
        return Ok(MonitorMatch::Primary);
    }
    if let Some((w, h)) = text.split_once(['x', 'X'])
        && let (Ok(w), Ok(h)) = (w.trim().parse(), h.trim().parse())
    {
        return Ok(MonitorMatch::Resolution(w, h));
    }
    let name = text.trim_start_matches(r"\\.\");
    let is_display = name
        .get(.."DISPLAY".len())
        .is_some_and(|p| p.eq_ignore_ascii_case("DISPLAY"));
    if is_display {
        return Ok(MonitorMatch::Name(name.to_owned()));
    }
    Err(format!(
        "monitor '{text}' isn't \"*\", \"primary\", a resolution like \
         \"3440x1440\" or a name like \"DISPLAY2\""
    ))
}

/// Parses an `anchor` value.
fn parse_anchor(text: &str) -> Result<Anchor, String> {
    Ok(match text {
        "top-left" => Anchor::TopLeft,
        "top" => Anchor::Top,
        "top-right" => Anchor::TopRight,
        "left" => Anchor::Left,
        "center" | "centre" => Anchor::Center,
        "right" => Anchor::Right,
        "bottom-left" => Anchor::BottomLeft,
        "bottom" => Anchor::Bottom,
        "bottom-right" => Anchor::BottomRight,
        other => return Err(format!("unknown anchor '{other}'")),
    })
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::{
        Anchor, DEFAULT_CONFIG, ImageChoice, Layout, MonitorInfo, MonitorMatch,
        Placement, Settings, parse,
    };
    use crate::backdrop::{Background, Fit, Wallpaper};
    use crate::color::Rgb;
    use crate::geometry::Rect;
    use crate::widgets::Widget;

    const MONITOR: Rect = Rect {
        left: 0,
        top: 0,
        right: 3440,
        bottom: 1440,
    };

    fn monitor(
        name: &str,
        width: i32,
        height: i32,
        primary: bool,
    ) -> MonitorInfo<'_> {
        MonitorInfo {
            name,
            width,
            height,
            primary,
        }
    }

    #[test]
    fn default_file_matches_built_in_defaults() {
        assert_eq!(parse(DEFAULT_CONFIG), Ok(Settings::default()));
    }

    #[test]
    fn empty_file_has_no_layouts() {
        let settings = parse("").unwrap();
        assert_eq!(settings.background, Background::Color(Rgb::BLACK));
        assert_eq!(settings.layouts, vec![]);
    }

    #[test]
    fn reads_a_two_monitor_layout() {
        let text = r##"
            background = "#102030"
            [[layout]]
            monitor = "3440x1440"
            size = 1.5
            [[layout.widget]]
            kind = "gpu"
            gpu = 2
            anchor = "top-left"
            x = 10
            y = 20
            [[layout]]
            monitor = "DISPLAY1"
        "##;
        let settings = parse(text).unwrap();
        assert_eq!(
            settings.background,
            Background::Color(Rgb {
                r: 0x10,
                g: 0x20,
                b: 0x30
            })
        );
        assert_eq!(
            settings.layouts[0],
            Layout {
                monitor: MonitorMatch::Resolution(3440, 1440),
                size: 1.5,
                widgets: vec![Placement {
                    widget: Widget::Gpu(1),
                    anchor: Anchor::TopLeft,
                    offset: (10.0, 20.0),
                }],
                background: None,
                image: ImageChoice::Inherit,
                image_fit: None,
                image_zoom: None,
                image_offset: (0.0, 0.0),
            }
        );
        assert_eq!(
            settings.layouts[1].monitor,
            MonitorMatch::Name("DISPLAY1".into())
        );
    }

    #[test]
    fn reads_backgrounds_and_pictures() {
        // An absolute path and a base folder as this system spells them.
        let (city, base) = if cfg!(windows) {
            (r"C:\walls\city.png", r"C:\config")
        } else {
            ("/walls/city.png", "/config")
        };
        let text = format!(
            r##"
            background = "Transparent"
            image = 'pictures\sky.jpg'
            image_fit = "fit"
            [[layout]]
            monitor = "DISPLAY2"
            background = "#FFFFFF"
            image = '{city}'
            image_fit = "centre"
            image_zoom = 1.25
            image_offset = [10, -4.5]
            [[layout]]
            monitor = "primary"
            image = ""
            [[layout]]
            monitor = "*"
        "##
        );
        let settings = parse(&text).unwrap().with_base_dir(Path::new(base));
        let sky = Path::new(base).join("pictures").join("sky.jpg");
        assert_eq!(settings.background, Background::Transparent);
        assert_eq!(settings.image.as_deref(), Some(sky.as_path()));
        assert_eq!(settings.image_fit, Fit::Contain);
        let [second, primary, rest] = settings.layouts.as_slice() else {
            panic!("three layouts");
        };
        let white = Background::Color(Rgb {
            r: 255,
            g: 255,
            b: 255,
        });
        let own = settings.backdrop_for(Some(second));
        assert_eq!(own.background, white);
        assert_eq!(
            own.image,
            Some(Wallpaper {
                path: city.into(),
                fit: Fit::Center,
                zoom: Some(1.25),
                offset: (10.0, -4.5),
            })
        );
        let off = settings.backdrop_for(Some(primary));
        assert_eq!(off.background, Background::Transparent);
        assert_eq!(off.image, None);
        let inherited = Some(Wallpaper {
            path: sky,
            fit: Fit::Contain,
            zoom: None,
            offset: (0.0, 0.0),
        });
        assert_eq!(settings.backdrop_for(Some(rest)).image, inherited);
        assert_eq!(settings.backdrop_for(None).image, inherited);
    }

    #[test]
    fn explains_mistakes() {
        let error = |text: &str| parse(text).unwrap_err().0;
        assert!(error("background = \"red\"").contains("background"));
        assert!(error("image_fit = \"tile\"").contains("image_fit"));
        assert_eq!(parse("image_fit = \"span\"").unwrap().image_fit, Fit::Span);
        let layout = |extra: &str| error(&format!("[[layout]]\n{extra}"));
        assert!(layout("image_zoom = 20").contains("image_zoom"));
        assert!(layout("image_offset = [1]").contains("image_offset"));
        assert!(
            error(
                "[[layout]]
background = \"clear\""
            )
            .contains("layout 1")
        );
        assert!(error("colour = 1").contains("colour"));
        assert!(
            error("[[layout]]\nmonitor = \"left one\"").contains("layout 1")
        );
        assert!(error("[[layout]]\nsize = 9").contains("size"));
        let bad_kind = "[[layout]]\n[[layout.widget]]\nkind = \"clock\"";
        assert!(error(bad_kind).contains("widget 1: unknown kind 'clock'"));
        let no_number = "[[layout]]\n[[layout.widget]]\nkind = \"gpu\"";
        assert!(error(no_number).contains("gpu = 1"));
        let bad_anchor =
            "[[layout]]\n[[layout.widget]]\nkind = \"cpu\"\nanchor = \"up\"";
        assert!(error(bad_anchor).contains("anchor"));
    }

    #[test]
    fn reads_discord_settings() {
        let text = "[discord]
client_id = \"123\"
client_secret = \"s\"
                    friends = [\"9\"]";
        let discord = parse(text).unwrap().discord.unwrap();
        assert_eq!(discord.redirect_uri, "http://localhost");
        assert_eq!(discord.friends, ["9"]);
        assert!(discord.bot_token.is_none());
        let bad = "[discord]
client_id = \"abc\"
client_secret = \"s\"";
        assert!(parse(bad).unwrap_err().0.contains("client_id"));
    }

    #[test]
    fn middle_anchors_default_to_centred() {
        let text = "[[layout]]
[[layout.widget]]
kind = \"cpu\"
                    anchor = \"top\"
[[layout.widget]]
kind = \"cpu\"
                    anchor = \"center\"
x = 5";
        let widgets = &parse(text).unwrap().layouts[0].widgets;
        assert_eq!(widgets[0].offset, (0.0, 64.0));
        assert_eq!(widgets[1].offset, (5.0, 0.0));
    }

    #[test]
    fn reads_panel_parts() {
        let text = "[panel]\ncpu = false\ngpus = [1]\n";
        let parts = parse(text).unwrap().parts;
        assert!(!parts.cpu && parts.network && parts.ram);
        assert_eq!(parts.gpus, Some(vec![0]));
        assert!(parse("[panel]\ngpus = [0]\n").is_err());
        assert!(parse("[panel]\nfan = true\n").is_err());
    }

    #[test]
    fn reads_network_graph_seconds() {
        assert_eq!(parse("").unwrap().net_seconds, 30);
        let text = "[network]\nseconds = 60\n";
        assert_eq!(parse(text).unwrap().net_seconds, 60);
        assert!(parse("[network]\nseconds = 5\n").is_err());
        assert!(parse("[network]\nseconds = -30\n").is_err());
        assert!(parse("[network]\nseconds = 301\n").is_err());
    }

    #[test]
    fn first_matching_layout_wins() {
        let text = r#"
            [[layout]]
            monitor = "primary"
            size = 2
            [[layout]]
            monitor = "*"
        "#;
        let settings = parse(text).unwrap();
        let main = monitor(r"\\.\DISPLAY1", 3840, 2160, true);
        let upper = monitor(r"\\.\DISPLAY5", 3440, 1440, false);
        assert_eq!(settings.layout_for(&main).map(|l| l.size), Some(2.0));
        assert_eq!(settings.layout_for(&upper).map(|l| l.size), Some(1.0));
    }

    #[test]
    fn matches_by_name_and_resolution() {
        let upper = monitor(r"\\.\DISPLAY5", 3440, 1440, false);
        assert!(MonitorMatch::Name("display5".into()).matches(&upper));
        assert!(!MonitorMatch::Name("DISPLAY1".into()).matches(&upper));
        assert!(MonitorMatch::Resolution(3440, 1440).matches(&upper));
        assert!(!MonitorMatch::Primary.matches(&upper));
    }

    #[test]
    fn places_against_each_anchor() {
        let size = (400, 300);
        let offset = (64, 32);
        let place =
            |anchor: Anchor| anchor.place(MONITOR, size, offset, MONITOR);
        let at = |r: Rect| (r.left, r.top);
        assert_eq!(at(place(Anchor::TopLeft)), (64, 32));
        assert_eq!(
            at(place(Anchor::BottomRight)),
            (3440 - 64 - 400, 1440 - 32 - 300)
        );
        assert_eq!(at(place(Anchor::Center)), (1520 + 64, 570 + 32));
        assert_eq!(at(place(Anchor::Top)), (1520 + 64, 32));
        assert_eq!(at(place(Anchor::Left)), (64, 570 + 32));
    }

    #[test]
    fn keeps_widgets_on_the_monitor() {
        // Above the taskbar: the work area is 48 pixels shorter.
        let work = Rect {
            bottom: 1440 - 48,
            ..MONITOR
        };
        let r = Anchor::BottomRight.place(work, (400, 300), (0, 0), MONITOR);
        assert_eq!(r.bottom, 1392);
        // A big offset can't push it off the monitor.
        let r =
            Anchor::TopLeft.place(MONITOR, (400, 300), (5000, -50), MONITOR);
        assert_eq!((r.left, r.top), (3040, 0));
        // Too big to fit: pinned to the top-left.
        let r =
            Anchor::BottomRight.place(MONITOR, (4000, 2000), (0, 0), MONITOR);
        assert_eq!((r.left, r.top), (0, 0));
    }
}
