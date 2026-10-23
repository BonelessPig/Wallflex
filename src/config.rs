//! Settings from `config.toml`: the background colour and which widgets
//! go where on each monitor. The file format is documented in
//! `config.default.toml`, which is also written for new users.

use std::fmt;

use serde::Deserialize;

use crate::color::Rgb;
use crate::geometry::Rect;
use crate::widgets::Widget;

/// The commented default config, written on first run.
pub const DEFAULT_CONFIG: &str = include_str!("../config.default.toml");

/// Default gap from the anchored edges, in design units.
const DEFAULT_OFFSET: f64 = 64.0;
/// Smallest and largest `size` accepted.
const SIZE_RANGE: (f64, f64) = (0.25, 4.0);

/// Validated settings.
#[derive(Clone, Debug, PartialEq)]
pub struct Settings {
    /// Background colour.
    pub background: Rgb,
    /// Layouts in file order; a monitor uses the first that matches.
    pub layouts: Vec<Layout>,
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
}

impl Default for Settings {
    /// The built-in defaults, the same as `config.default.toml`.
    fn default() -> Self {
        Self {
            background: Rgb::BLACK,
            layouts: vec![Layout {
                monitor: MonitorMatch::Any,
                size: 1.0,
                widgets: vec![Placement {
                    widget: Widget::Panel,
                    anchor: Anchor::BottomRight,
                    offset: (DEFAULT_OFFSET, DEFAULT_OFFSET),
                }],
            }],
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
    /// `"#RRGGBB"`.
    background: Option<String>,
    /// `[[layout]]` tables.
    #[serde(default)]
    layout: Vec<RawLayout>,
}

/// One `[[layout]]` as written.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawLayout {
    /// Which monitors.
    monitor: Option<String>,
    /// Scale factor.
    size: Option<f64>,
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
    let background = match raw.background {
        Some(hex) => Rgb::from_hex(&hex).ok_or_else(|| {
            ConfigError(format!("background: '{hex}' isn't a #RRGGBB colour"))
        })?,
        None => Rgb::BLACK,
    };
    let layouts = (1_u32..)
        .zip(raw.layout)
        .map(|(number, layout)| {
            parse_layout(layout)
                .map_err(|e| ConfigError(format!("layout {number}: {e}")))
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Settings {
        background,
        layouts,
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
    Ok(Layout {
        monitor,
        size,
        widgets,
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
    use super::{
        Anchor, DEFAULT_CONFIG, Layout, MonitorInfo, MonitorMatch, Placement,
        Settings, parse,
    };
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
        assert_eq!(settings.background, Rgb::BLACK);
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
            Rgb {
                r: 0x10,
                g: 0x20,
                b: 0x30
            }
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
            }
        );
        assert_eq!(
            settings.layouts[1].monitor,
            MonitorMatch::Name("DISPLAY1".into())
        );
    }

    #[test]
    fn explains_mistakes() {
        let error = |text: &str| parse(text).unwrap_err().0;
        assert!(error("background = \"red\"").contains("background"));
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
