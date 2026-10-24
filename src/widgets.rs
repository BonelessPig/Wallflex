//! The widgets: CPU thread columns, the network graph and the GPU rings,
//! redrawn natively from the user's wallpaper designs (`cpu-cores.js`,
//! `net-graph.js`, `gpu-rings.js` and their CSS).
//!
//! Sizes are in design units: 1 unit = 0.1% of the monitor's height (the
//! wallpaper's 0.1vh), which is also the line weight. Shapes go to a
//! [`Canvas`]; text comes back as [`Label`]s for GDI to draw on top.

mod discord;
mod motion;
mod visualizer;

pub use discord::{Action, Demo};
pub use motion::{Ease, Glide};

use std::collections::VecDeque;
use std::time::Duration;

use crate::canvas::{Area, Canvas, Paint, mix};
use crate::color::Rgb;
use crate::sensors::{
    BYTES_PER_GB, Gpu, Network, PERCENT_MAX, Snapshot, u64_to_f64,
};

/// Design units per monitor height.
pub const UNITS_PER_HEIGHT: f64 = 1_000.0;

/// The ink colour (the wallpaper's `currentColor`).
const INK: Rgb = Rgb {
    r: 255,
    g: 255,
    b: 255,
};
/// Opacity of secondary text (labels, meta).
const QUIET: f64 = 0.55;
/// Opacity of the CPU column outlines and the graph's baseline.
const TRACK: f64 = 0.25;
/// Opacity of the graph's dashed halfway line.
const GRID: f64 = 0.12;
/// The line weight, in units.
const WEIGHT: f64 = 1.0;

/// Width of one widget column (19.5vh).
const COLUMN_W: f64 = 195.0;
/// Gap between widget columns (4.25vh).
const COLUMN_GAP: f64 = 42.5;
/// Gap between widget rows (1.6vh).
const ROW_GAP: f64 = 16.0;
/// Heading text size (1.05vh).
const HEAD_SIZE: f64 = 10.5;
/// Where a widget's body starts, below its heading (heading + 1vh).
const BODY_TOP: f64 = 23.0;
/// Baseline of heading text.
const HEAD_BASELINE: f64 = 9.0;
/// Text that's missing.
const MISSING: &str = "--";

/// CPU: height of the columns area (7.5vh).
const CPU_COLS_H: f64 = 75.0;
/// CPU: gap between cores (0.7vh).
const CPU_CORE_GAP: f64 = 7.0;
/// CPU: gap between a core's two threads (0.25vh).
const CPU_PAIR_GAP: f64 = 2.5;
/// CPU: gap between columns and their label (0.6vh).
const CPU_LABEL_GAP: f64 = 6.0;
/// CPU: core label size (0.9vh).
const CPU_LABEL_SIZE: f64 = 9.0;
/// CPU: inset of the fill inside its outline (0.2vh).
const CPU_FILL_INSET: f64 = 2.0;
/// Windows numbers a core's two threads next to each other.
const THREADS_PER_CORE: usize = 2;

/// How quickly the graph's scale follows a new peak (time constant).
const NET_PEAK_EASE: Duration = Duration::from_millis(400);
/// Network readings a second at most (the graph's resolution).
const NET_READINGS_PER_SECOND: usize = 4;
/// Network: seconds of history shown unless the config says otherwise.
pub const NET_DEFAULT_SECONDS: u32 = 30;
/// Network: opacity of the fill under the download line.
const NET_DOWN_FILL: f64 = 0.3;
/// Network: opacity of the fill under the upload line.
const NET_UP_FILL: f64 = 0.25;
/// Network: the rounded line has a point at least this often (units),
/// about a pixel at 4K, so its curves look smooth.
const NET_CURVE_STEP: f64 = 0.5;
/// Network: most points the rounded line adds between two readings.
const NET_CURVE_MAX_STEPS: u32 = 8;
/// Network: plot top, inside the graph.
const NET_TOP: f64 = 2.0;
/// Network: plot height.
const NET_H: f64 = 70.0;
/// Network: graph height including its labels.
const NET_GRAPH_H: f64 = 86.0;
/// Network: graph text size.
const NET_TEXT: f64 = 8.0;
/// Network: dash and gap of the halfway line.
const NET_DASH: (f64, f64) = (2.0, 3.0);
/// Network: the scale never drops below 1 Mbps, so idle traffic is flat.
const NET_MIN_PEAK_KBPS: f64 = 1_000.0;
/// Network: kbit/s per Mbit/s.
const KBPS_PER_MBPS: f64 = 1_000.0;
/// Network: drop the decimal at 100 Mbps and up.
const WHOLE_MBPS_FROM: f64 = 100.0;
/// Network: upload colour.
const NET_UP_COLOR: Rgb = Rgb {
    r: 0xA8,
    g: 0xE6,
    b: 0xFF,
};
/// Network: width of one heading slot. Narrower than the wallpaper's
/// 7.3vh because Bahnschrift is wider than its font.
const NET_SLOT_W: f64 = 70.0;
/// Network: heading readout size, a little under the heading so both
/// readouts fit beside "NETWORK".
const NET_META_SIZE: f64 = 9.5;
/// Network: colour key width (0.9vh) and gap after it.
const KEY_W: f64 = 9.0;
/// Gap after a colour key.
const KEY_GAP: f64 = 3.0;

/// Rings: chart size (7.8vh).
const RING_SIZE: f64 = 78.0;
/// Rings: the wallpaper's SVG viewBox size, for converting its numbers.
const RING_VIEW: f64 = 168.0;
/// Rings: outer radius in viewBox units.
const RING_OUTER_R: f64 = 78.0;
/// Rings: gap between rings in viewBox units.
const RING_GAP: f64 = 11.0;
/// Rings: space above the charts for the GPU names (1.2vh).
const RING_NAME_ROOM: f64 = 12.0;
/// Rings: gap between ring and legend (0.8vh).
const LEGEND_GAP: f64 = 8.0;
/// Rings: legend row height.
const LEGEND_ROW_H: f64 = 17.0;
/// Rings: legend label column width (3.6vh) and column gap (0.45vh).
const LEGEND_LABEL_W: f64 = 36.0;
/// Gap between legend columns.
const LEGEND_COL_GAP: f64 = 4.5;
/// Rings: model name size and baseline, converted from the viewBox.
const MODEL_SIZE: f64 = 24.0 * RING_SIZE / RING_VIEW;
/// Baseline of the model name, relative to the ring's top.
const MODEL_BASELINE: f64 = -8.0 * RING_SIZE / RING_VIEW;
/// Series name size, converted from the viewBox.
const SERIES_SIZE: f64 = 15.0 * RING_SIZE / RING_VIEW;
/// Baseline of the series name, relative to the ring's top.
const SERIES_BASELINE: f64 = 11.0 * RING_SIZE / RING_VIEW;
/// Rings: charts per row (two fit in the block's width).
const RINGS_PER_ROW: usize = 2;
/// Rings: empty temperature ring, °C (a GPU never runs cooler).
const TEMP_MIN: f64 = 20.0;
/// Rings: full temperature ring if the driver reports no limit, °C.
const TEMP_MAX_FALLBACK: f64 = 100.0;
/// Rings: full fan ring if the driver reports no limit, RPM.
const FAN_MAX_FALLBACK: f64 = 3_000.0;

/// The rings, outer to inner.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Ring {
    /// GPU use.
    Load,
    /// VRAM in use.
    Vram,
    /// Temperature.
    Temp,
    /// Power vs the card's limit.
    Power,
    /// Fan speed.
    Fan,
}

impl Ring {
    /// Outer to inner.
    const ALL: [Self; 5] =
        [Self::Load, Self::Vram, Self::Temp, Self::Power, Self::Fan];

    /// Legend label.
    fn label(self) -> &'static str {
        match self {
            Self::Load => "LOAD",
            Self::Vram => "VRAM",
            Self::Temp => "TEMP",
            Self::Power => "POWER",
            Self::Fan => "FAN",
        }
    }

    /// Ring colour.
    fn color(self) -> Rgb {
        let (r, g, b) = match self {
            Self::Load => (0xA8, 0xE6, 0xFF),
            Self::Vram => (0xCD, 0xBD, 0xFF),
            Self::Temp => (0xFF, 0xD3, 0xA1),
            Self::Power => (0xFF, 0xB0, 0xB8),
            Self::Fan => (0xB4, 0xF0, 0xCC),
        };
        Rgb { r, g, b }
    }
}

/// How text lines up with its x position.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Align {
    /// x is the left edge.
    Left,
    /// x is the right edge.
    Right,
    /// x is the centre.
    Center,
}

/// A piece of text for GDI to draw, in pixels relative to the widgets.
#[derive(Clone, Debug, PartialEq)]
pub struct Label {
    /// Horizontal position (see `align`).
    pub x: f64,
    /// Baseline.
    pub y: f64,
    /// Font size (em height), in pixels.
    pub size: f64,
    /// Colour, already mixed with the background for its opacity.
    pub color: Rgb,
    /// Alignment.
    pub align: Align,
    /// The text.
    pub text: String,
    /// Only draw inside this area (pixels), if set: for text that
    /// scrolls through a box.
    pub clip: Option<Area>,
}

/// A widget that can be placed on its own.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Widget {
    /// Everything below in one block: CPU and network side by side,
    /// the GPU rings under them, then RAM and SSD temperatures.
    Panel,
    /// CPU thread columns.
    Cpu,
    /// Network graph.
    Network,
    /// Every GPU's ring chart, two per row.
    Gpus,
    /// One GPU's ring chart (0 = the first GPU, in PCI slot order).
    Gpu(usize),
    /// RAM use with a bar, and SSD temperatures.
    Info,
    /// Discord: your call, notifications and more.
    Discord,
    /// Music visualizer: bars for what's playing.
    Visualizer,
}

/// The graph's scale counts as settled within this share of the peak.
const NET_PEAK_SETTLED: f64 = 0.002;
/// Room around a moving part, so its smoothed edges repaint too.
const MOVING_MARGIN: f64 = 1.0;
/// Top of the RAM bar, under the RAM text.
const RAM_BAR_TOP: f64 = HEAD_SIZE * 1.4;
/// Thickness of the RAM bar.
const RAM_BAR_H: f64 = 3.0;
/// Gap between the RAM bar's sections, one per stick (0.3vh).
const RAM_BAR_GAP: f64 = 3.0;
/// Height of the RAM/SSD row, bar included.
const INFO_H: f64 = RAM_BAR_TOP + RAM_BAR_H;
/// The RAM bar's section colours, one per stick, as the wallpaper's.
const STICK_COLORS: [Rgb; 4] = [
    Rgb {
        r: 0xA8,
        g: 0xE6,
        b: 0xFF,
    },
    Rgb {
        r: 0xCD,
        g: 0xBD,
        b: 0xFF,
    },
    Rgb {
        r: 0xFF,
        g: 0xD3,
        b: 0xA1,
    },
    Rgb {
        r: 0xB4,
        g: 0xF0,
        b: 0xCC,
    },
];
/// Height of one GPU ring chart, names included.
const GPU_CHART_H: f64 = RING_NAME_ROOM + RING_SIZE;

/// A clickable area of a drawn widget, in pixels from its top-left.
#[derive(Clone, Debug, PartialEq)]
pub struct Hit {
    /// The area.
    pub area: Area,
    /// What a click there does.
    pub action: Action,
}

/// What drawing a widget produced besides its pixels.
#[derive(Default)]
pub struct Drawing {
    /// Text for GDI to draw on top.
    pub labels: Vec<Label>,
    /// Clickable areas, later ones on top.
    pub hits: Vec<Hit>,
}

/// What the widgets show: the latest reading plus the network history.
#[derive(Default)]
pub struct Dashboard {
    /// The latest reading.
    snapshot: Snapshot,
    /// The last [`Dashboard::net_span`] network readings, oldest first.
    net: VecDeque<Network>,
    /// The Discord widget's state.
    discord: discord::DiscordUi,
    /// The CPU columns' glide to the latest reading.
    motion: motion::Motion,
    /// The GPU rings' and RAM bar's glide (their readings come less
    /// often, so their glides can be longer).
    slow_motion: motion::Motion,
    /// The music visualizer's bars.
    visualizer: visualizer::Visualizer,
    /// Network readings per sample kept: 2 when readings come twice a
    /// second, so the graph still spans its full time.
    net_copies: usize,
    /// How long the graph takes to slide one reading along; `None` when
    /// it steps.
    net_scroll: Option<Duration>,
    /// How far the newest reading has slid in, 0 to 1 (1 = in place).
    net_phase: f64,
    /// The graph's scale as drawn, easing toward the real peak.
    net_peak: f64,
    /// Internet download speed from the config (kbit/s), if set.
    net_limit_kbps: Option<f64>,
    /// Seconds of history the graph shows; 0 means
    /// [`NET_DEFAULT_SECONDS`].
    net_seconds: u32,
    /// Which parts show.
    parts: Parts,
    /// Glide frames so far.
    frame: u32,
    /// The graph moves every this many frames.
    net_every: u32,
    /// The rings and RAM bar move every this many frames.
    slow_every: u32,
    /// Time the graph has waited for its frame.
    net_wait: Duration,
    /// Time the rings and RAM bar have waited for theirs.
    slow_wait: Duration,
    /// Which groups moved on the last frame: CPU columns, graph, rings
    /// and RAM bar. Only they need repainting.
    stepped: (bool, bool, bool),
}

/// Which parts the panel shows (the config's `[panel]`). By default
/// everything detected on this PC.
#[derive(Clone, Debug, PartialEq, Eq)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "one independent switch per part, as the config has them"
)]
pub struct Parts {
    /// CPU thread columns.
    pub cpu: bool,
    /// Network graph.
    pub network: bool,
    /// RAM line and bar.
    pub ram: bool,
    /// SSD temperatures.
    pub ssd: bool,
    /// Which GPUs (0 = the first); `None` shows every one detected.
    pub gpus: Option<Vec<usize>>,
}

impl Default for Parts {
    fn default() -> Self {
        Self {
            cpu: true,
            network: true,
            ram: true,
            ssd: true,
            gpus: None,
        }
    }
}

/// Where the panel's parts go, from [`Parts`] and the hardware found.
struct PanelLayout {
    /// The CPU block's left edge, if shown.
    cpu: Option<f64>,
    /// The network block's left edge, if shown.
    network: Option<f64>,
    /// Top of the GPU charts.
    gpus_top: f64,
    /// The GPUs shown, by index into the reading.
    gpus: Vec<usize>,
    /// Top of the RAM/SSD row, if shown.
    info_top: Option<f64>,
    /// The panel's height.
    height: f64,
}

/// How the bars, rings and graph move, from the performance level.
#[derive(Clone, Copy, Debug)]
pub struct Pace {
    /// The CPU columns' glide.
    pub glide: Glide,
    /// The GPU rings' and RAM bar's glide.
    pub slow_glide: Glide,
    /// How long the graph takes to slide one reading along; `None`
    /// steps.
    pub net_scroll: Option<Duration>,
    /// Quick readings a second (for the graph's span).
    pub readings_per_second: usize,
    /// The graph moves every this many glide frames.
    pub net_every: u32,
    /// The rings and RAM bar move every this many glide frames.
    pub slow_every: u32,
}

impl Dashboard {
    /// Takes in a new reading.
    pub fn update(&mut self, snapshot: Snapshot) {
        let before = self.shown();
        if let Some(network) = snapshot.network {
            self.net_phase = if self.net_scroll.is_some() { 0.0 } else { 1.0 };
            for _ in 0..self.net_copies.max(1) {
                self.net.push_back(network);
            }
            self.trim_net();
        }
        self.snapshot = snapshot;
        let target = self.target();
        // Each group restarts only when its own values changed, so a
        // quick (CPU-only) reading doesn't cut the rings' glide short.
        let ram_moved = (before.ram - target.ram).abs() > f64::EPSILON;
        if before.rings != target.rings || ram_moved {
            self.slow_motion.restart(before.clone());
        }
        if before.cores == target.cores {
            self.motion.settle();
        } else {
            self.motion.restart(before);
        }
    }

    /// Moves the bars and rings on by `elapsed`. Returns whether they're
    /// still moving (repaint the [`Dashboard::moving_areas`] until not).
    pub fn advance(&mut self, elapsed: Duration) -> bool {
        self.frame = self.frame.wrapping_add(1);
        // Each group moves (and repaints) on its own frames: the CPU
        // columns every frame, the slower graph and rings less often,
        // catching up on the time in between.
        self.stepped.0 = self.motion.is_moving();
        let _ = self.motion.advance(elapsed);
        self.net_wait = self.net_wait.saturating_add(elapsed);
        self.stepped.1 = false;
        if self.frame.is_multiple_of(self.net_every.max(1)) {
            self.stepped.1 = self.net_moving();
            let waited = std::mem::take(&mut self.net_wait);
            self.advance_net(waited);
        }
        self.slow_wait = self.slow_wait.saturating_add(elapsed);
        self.stepped.2 = false;
        if self.frame.is_multiple_of(self.slow_every.max(1)) {
            self.stepped.2 = self.slow_motion.is_moving();
            let waited = std::mem::take(&mut self.slow_wait);
            let _ = self.slow_motion.advance(waited);
        }
        self.is_moving()
    }

    /// Slides the network graph and eases its scale on by `elapsed`.
    fn advance_net(&mut self, elapsed: Duration) {
        if let Some(scroll) = self.net_scroll.filter(|s| !s.is_zero()) {
            let step = elapsed.as_secs_f64() / scroll.as_secs_f64();
            self.net_phase = (self.net_phase + step).min(1.0);
            // Ease the scale toward the peak, the same at any frame rate.
            let ease = 1.0
                - (-elapsed.as_secs_f64() / NET_PEAK_EASE.as_secs_f64()).exp();
            self.net_peak += (self.net_peak_target() - self.net_peak) * ease;
        }
    }

    /// Whether the bars and rings are gliding to a new reading.
    pub fn is_moving(&self) -> bool {
        self.motion.is_moving()
            || self.slow_motion.is_moving()
            || self.net_moving()
    }

    /// Whether the network graph is still sliding or rescaling.
    fn net_moving(&self) -> bool {
        self.net_scroll.is_some()
            && (self.net_phase < 1.0
                || (self.net_peak - self.net_peak_target()).abs()
                    > self.net_peak_target() * NET_PEAK_SETTLED)
    }

    /// The highest reading in the graph: the scale it's heading for.
    fn net_peak_target(&self) -> f64 {
        self.net
            .iter()
            .flat_map(|n| [n.down_kbps, n.up_kbps])
            .fold(NET_MIN_PEAK_KBPS, f64::max)
    }

    /// The scale to draw the graph at.
    fn net_peak_shown(&self) -> f64 {
        if self.net_scroll.is_some() && self.net_peak > 0.0 {
            self.net_peak
        } else {
            self.net_peak_target()
        }
    }

    /// Sets how the bars, rings and graph move ([`Pace`]).
    pub fn set_pace(&mut self, pace: Pace) {
        self.motion.set_glide(pace.glide);
        self.slow_motion.set_glide(pace.slow_glide);
        self.net_scroll = pace.net_scroll;
        self.net_phase = 1.0;
        self.net_peak = self.net_peak_target();
        self.net_every = pace.net_every.max(1);
        self.slow_every = pace.slow_every.max(1);
        let per_second =
            pace.readings_per_second.clamp(1, NET_READINGS_PER_SECOND);
        self.net_copies =
            NET_READINGS_PER_SECOND.checked_div(per_second).unwrap_or(1);
    }

    /// Sets which parts show.
    pub fn set_parts(&mut self, parts: Parts) {
        self.parts = parts;
    }

    /// The GPUs to show, by index into the reading.
    fn shown_gpus(&self) -> Vec<usize> {
        let count = self.snapshot.gpus.len();
        match &self.parts.gpus {
            None => (0..count).collect(),
            Some(wanted) => {
                wanted.iter().copied().filter(|i| *i < count).collect()
            }
        }
    }

    /// Where the panel's parts go. The top row holds the CPU and network
    /// blocks that are shown, left to right; then the GPU charts; then
    /// the RAM/SSD row. Hidden parts leave no gap.
    fn panel_layout(&self) -> PanelLayout {
        let parts = &self.parts;
        let mut x = 0.0;
        let cpu = parts.cpu.then(|| {
            let left = x;
            x += COLUMN_W + COLUMN_GAP;
            left
        });
        let network = parts.network.then_some(x);
        let mut y = 0.0;
        if cpu.is_some() || network.is_some() {
            let row = (BODY_TOP + CPU_COLS_H).max(BODY_TOP + NET_GRAPH_H);
            y = row + ROW_GAP;
        }
        let gpus = self.shown_gpus();
        let gpus_top = y;
        let (_, gpus_h) = gpus_size(gpus.len());
        if gpus_h > 0.0 {
            y = gpus_top + gpus_h + ROW_GAP;
        }
        let info_top = (parts.ram || parts.ssd).then_some(y);
        let height = match info_top {
            Some(top) => top + INFO_H,
            None => (y - ROW_GAP).max(0.0),
        };
        PanelLayout {
            cpu,
            network,
            gpus_top,
            gpus,
            info_top,
            height,
        }
    }

    /// Sets how many seconds of history the network graph shows.
    pub fn set_net_seconds(&mut self, seconds: u32) {
        self.net_seconds = seconds;
        self.trim_net();
        self.net_peak = self.net_peak_target();
    }

    /// The graph's time span in seconds.
    fn net_seconds(&self) -> u32 {
        if self.net_seconds == 0 {
            NET_DEFAULT_SECONDS
        } else {
            self.net_seconds
        }
    }

    /// Samples the graph spans: four a second.
    fn net_span(&self) -> usize {
        usize::try_from(self.net_seconds())
            .unwrap_or(usize::MAX)
            .saturating_mul(NET_READINGS_PER_SECOND)
    }

    /// Drops readings older than the graph shows, keeping one extra
    /// sample to slide in from the left edge.
    fn trim_net(&mut self) {
        let keep = self.net_span().saturating_add(1);
        while self.net.len() > keep {
            let _oldest = self.net.pop_front();
        }
    }

    /// Sets the internet plan's download speed (kbit/s) for the graph's
    /// colours; `None` uses the network adapter's speed alone.
    pub fn set_net_limit(&mut self, kbps: Option<f64>) {
        self.net_limit_kbps = kbps;
    }

    /// The download speed the graph's colours reach red at: the
    /// adapter's speed or the internet plan's, whichever is lower.
    fn net_max_kbps(&self) -> Option<f64> {
        let link = self.snapshot.network.and_then(|n| n.link_kbps);
        match (link, self.net_limit_kbps) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        }
        .filter(|max| *max > 0.0)
    }

    /// The whole PC's CPU use in the latest reading (%).
    pub fn cpu_total(&self) -> Option<f64> {
        self.snapshot.cpu.total
    }

    /// Drops downloaded pictures (to free memory); they're fetched again
    /// when needed.
    pub fn clear_images(&mut self) {
        self.discord.clear_images();
    }

    /// Jumps straight to the latest reading (for `--preview`).
    pub fn settle(&mut self) {
        self.motion.settle();
        self.slow_motion.settle();
        self.net_phase = 1.0;
        self.net_peak = self.net_peak_target();
    }

    /// One visualizer frame from the latest audio (mono, newest last, at
    /// `rate` Hz), `elapsed` after the last (for a scrolling title).
    pub fn feed_audio(
        &mut self,
        samples: &[f32],
        rate: u32,
        elapsed: Duration,
    ) {
        self.visualizer.feed(samples, rate);
        self.visualizer.scroll(elapsed);
    }

    /// What's playing changed.
    pub fn set_now_playing(&mut self, change: crate::media::MediaChange) {
        self.visualizer.set_now_playing(change);
    }

    /// Whether the visualizer's bars have come to rest.
    pub fn visualizer_resting(&self) -> bool {
        self.visualizer.is_resting()
    }

    /// Drops the visualizer's bars at once.
    pub fn rest_visualizer(&mut self) {
        self.visualizer.rest();
    }

    /// The parts of `widget` the visualizer's frames change, in its
    /// units: a visualizer's bars and a scrolling title, nothing
    /// elsewhere.
    pub fn visualizer_areas(&self, widget: Widget) -> Vec<Area> {
        if widget == Widget::Visualizer {
            self.visualizer.moving_areas()
        } else {
            Vec::new()
        }
    }

    /// Sample visualizer bars, for `--preview`.
    pub fn show_visualizer_demo(&mut self) {
        self.visualizer.show_demo();
    }

    /// The bars' and rings' values in the latest reading.
    fn target(&self) -> motion::Levels {
        let rings = self
            .snapshot
            .gpus
            .iter()
            .flat_map(|gpu| {
                Ring::ALL
                    .iter()
                    .map(|&ring| ring_reading(gpu, ring).0.unwrap_or(0.0))
            })
            .collect();
        let ram = self
            .snapshot
            .memory
            .filter(|m| m.total_bytes > 0)
            .map_or(0.0, |m| {
                u64_to_f64(m.used_bytes) / u64_to_f64(m.total_bytes)
            });
        motion::Levels {
            cores: self.snapshot.cpu.per_core.clone(),
            rings,
            ram,
        }
    }

    /// The bars' and rings' values to draw now.
    fn shown(&self) -> motion::Levels {
        let target = self.target();
        let slow = self.slow_motion.shown(&target);
        motion::Levels {
            cores: self.motion.shown(&target).cores,
            ..slow
        }
    }

    /// The parts of `widget` a quick reading (CPU and network only)
    /// changes, in its own units.
    pub fn quick_areas(&self, widget: Widget) -> Vec<Area> {
        let (cpu, net) = (cpu_block_area(), net_block_area());
        match widget {
            Widget::Panel => {
                let layout = self.panel_layout();
                let mut areas = Vec::new();
                areas.extend(layout.cpu.map(|x| shift_x(cpu, x)));
                areas.extend(layout.network.map(|x| shift_x(net, x)));
                areas
            }
            Widget::Cpu => vec![cpu],
            Widget::Network => vec![net],
            Widget::Gpus
            | Widget::Gpu(_)
            | Widget::Info
            | Widget::Discord
            | Widget::Visualizer => Vec::new(),
        }
    }

    /// Where `widget` has parts that glide, in its own units: the CPU
    /// columns, the GPU rings and the RAM bar. Repainting just these is
    /// enough while [`Dashboard::is_moving`].
    pub fn moving_areas(&self, widget: Widget) -> Vec<Area> {
        let gpus = self.snapshot.gpus.len();
        let (fast, net, slow) = self.stepped;
        let mut areas = Vec::new();
        match widget {
            Widget::Panel => {
                let layout = self.panel_layout();
                if fast {
                    areas.extend(
                        layout.cpu.map(|x| shift_x(cpu_columns_area(), x)),
                    );
                }
                if net {
                    areas.extend(
                        layout.network.map(|x| shift_x(net_plot_area(), x)),
                    );
                }
                if slow {
                    let top = layout.gpus_top;
                    areas.extend((0..layout.gpus.len()).map(|spot| {
                        shift(ring_area(gpu_grid_spot(spot)), top)
                    }));
                    if self.parts.ram {
                        areas.extend(
                            layout
                                .info_top
                                .map(|top| shift(ram_bar_area(), top)),
                        );
                    }
                }
            }
            Widget::Cpu if fast => areas.push(cpu_columns_area()),
            Widget::Network if net => areas.push(net_plot_area()),
            Widget::Gpus if slow => {
                let shown = self.shown_gpus().len();
                areas.extend((0..shown).map(|i| ring_area(gpu_grid_spot(i))));
            }
            Widget::Gpu(index) if slow && index < gpus => {
                areas.push(ring_area((0.0, RING_NAME_ROOM)));
            }
            Widget::Info if slow && self.parts.ram => {
                areas.push(ram_bar_area());
            }
            _ => {}
        }
        areas
    }

    /// A widget's size in design units. (0, 0) when it has nothing to
    /// show, such as a GPU that isn't there.
    pub fn size(&self, widget: Widget) -> (f64, f64) {
        match widget {
            Widget::Panel => {
                let layout = self.panel_layout();
                let width = if layout.height > 0.0 { PANEL_W } else { 0.0 };
                (width, layout.height)
            }
            Widget::Cpu => (COLUMN_W, BODY_TOP + CPU_COLS_H),
            Widget::Network => (COLUMN_W, BODY_TOP + NET_GRAPH_H),
            Widget::Gpus => gpus_size(self.shown_gpus().len()),
            Widget::Gpu(index) => {
                if index < self.snapshot.gpus.len() {
                    (COLUMN_W, GPU_CHART_H)
                } else {
                    (0.0, 0.0)
                }
            }
            Widget::Info => (PANEL_W, INFO_H),
            Widget::Visualizer => self.visualizer.size(),
            Widget::Discord => self.discord.size(),
        }
    }

    /// Draws `widget` into `canvas` (already sized to it and cleared to
    /// `background`) at `scale` pixels per unit. Returns the text to draw
    /// on top and the clickable areas.
    pub fn draw(
        &self,
        widget: Widget,
        canvas: &mut Canvas<'_>,
        scale: f64,
        background: Rgb,
    ) -> Drawing {
        let mut pen = Pen {
            canvas,
            scale,
            background,
            origin: (0.0, 0.0),
            labels: Vec::new(),
            hits: Vec::new(),
        };
        match widget {
            Widget::Panel => self.draw_panel(&mut pen),
            Widget::Cpu => self.draw_cpu(&mut pen),
            Widget::Network => self.draw_net(&mut pen),
            Widget::Gpus => self.draw_gpus(&mut pen),
            Widget::Gpu(index) => {
                if let Some(gpu) = self.snapshot.gpus.get(index) {
                    let levels = self.shown();
                    let rings = gpu_rings(&levels.rings, index);
                    draw_ring_chart(
                        &mut pen,
                        (0.0, RING_NAME_ROOM),
                        gpu,
                        rings,
                    );
                }
            }
            Widget::Info => self.draw_info(&mut pen),
            Widget::Visualizer => self.visualizer.draw(&mut pen),
            Widget::Discord => self.discord.draw(&mut pen),
        }
        Drawing {
            labels: pen.labels,
            hits: pen.hits,
        }
    }

    /// Notes the control under the pointer, for hover highlights.
    /// Returns whether the Discord widget needs repainting.
    pub fn set_hover(&mut self, hover: Option<Action>) -> bool {
        self.discord.set_hover(hover)
    }

    /// Handles a click on a widget. Returns a command for the Discord
    /// session, if the click needs one.
    pub fn click(
        &mut self,
        action: &Action,
    ) -> Option<crate::discord::session::Command> {
        self.discord.click(action)
    }

    /// Whether Discord is set up; the Discord widget hides otherwise.
    pub fn set_discord_configured(&mut self, configured: bool, bot: bool) {
        self.discord.set_configured(configured, bot);
    }

    /// Takes in a new view from the Discord bot.
    pub fn update_bot(&mut self, view: Option<crate::discord::bot::BotView>) {
        self.discord.update_bot(view);
    }

    /// Pictures the Discord widget needs but doesn't have yet. Each URL
    /// is returned once.
    pub fn wanted_images(&mut self) -> Vec<String> {
        self.discord.wanted_images()
    }

    /// A command for the Discord session when the SERVER page needs a
    /// different server's channels.
    pub fn discord_request(
        &mut self,
    ) -> Option<crate::discord::session::Command> {
        self.discord.server_request()
    }

    /// Hands over a downloaded picture.
    pub fn add_image(&mut self, url: String, image: crate::image::Image) {
        self.discord.add_image(url, image);
    }

    /// Fills the Discord widget with sample data, for `--preview`.
    pub fn show_discord_demo(&mut self, demo: Demo) {
        self.discord.show_demo(demo);
    }

    /// Takes in a new view from the Discord session.
    pub fn update_discord(&mut self, view: crate::discord::model::DiscordView) {
        self.discord.update(view);
    }

    /// The panel: CPU and network side by side, GPUs under them, then
    /// the RAM/SSD line.
    fn draw_panel(&self, pen: &mut Pen<'_, '_>) {
        let layout = self.panel_layout();
        if let Some(x) = layout.cpu {
            pen.origin = (x, 0.0);
            self.draw_cpu(pen);
        }
        if let Some(x) = layout.network {
            pen.origin = (x, 0.0);
            self.draw_net(pen);
        }
        pen.origin = (0.0, layout.gpus_top);
        self.draw_gpus(pen);
        if let Some(top) = layout.info_top {
            pen.origin = (0.0, top);
            self.draw_info(pen);
        }
        pen.origin = (0.0, 0.0);
    }

    /// The CPU thread columns, at the top left.
    fn draw_cpu(&self, pen: &mut Pen<'_, '_>) {
        let cpu = &self.snapshot.cpu;
        pen.text(
            (0.0, HEAD_BASELINE),
            HEAD_SIZE,
            1.0,
            Align::Left,
            "CPU THREADS",
        );
        let threads = cpu.per_core.len();
        if threads == 0 {
            pen.text(
                (COLUMN_W, HEAD_BASELINE),
                HEAD_SIZE,
                QUIET,
                Align::Right,
                "NO DATA",
            );
            return;
        }
        let layout = core_layout(&cpu.cores, threads);
        let cores = layout.len();
        let clock = cpu
            .clock_ghz
            .map_or_else(String::new, |ghz| format!("{ghz:.2} GHZ · "));
        let meta = format!("{clock}{cores}C / {threads}T");
        pen.text(
            (COLUMN_W, HEAD_BASELINE),
            HEAD_SIZE,
            QUIET,
            Align::Right,
            &meta,
        );

        let levels = self.shown();
        let core_n = count_to_f64(cores);
        let core_w = (COLUMN_W - CPU_CORE_GAP * (core_n - 1.0)) / core_n;
        let col_h = cpu_column_height();

        for (core, group) in (1_u32..).zip(&layout) {
            let core_x = (f64::from(core) - 1.0) * (core_w + CPU_CORE_GAP);
            let per_core_n = count_to_f64(group.len());
            let col_w =
                (core_w - CPU_PAIR_GAP * (per_core_n - 1.0)) / per_core_n;
            for (thread, &index) in (0_u32..).zip(group) {
                let load = levels.cores.get(index).copied().unwrap_or(0.0);
                let x = core_x + f64::from(thread) * (col_w + CPU_PAIR_GAP);
                let column = Area {
                    x0: x,
                    y0: BODY_TOP,
                    x1: x + col_w,
                    y1: BODY_TOP + col_h,
                };
                pen.stroke(column, TRACK);
                let fill_h =
                    (col_h - 2.0 * CPU_FILL_INSET) * load / PERCENT_MAX;
                let bottom = column.y1 - CPU_FILL_INSET;
                let fill = Area {
                    x0: column.x0 + CPU_FILL_INSET,
                    y0: bottom - fill_h,
                    x1: column.x1 - CPU_FILL_INSET,
                    y1: bottom,
                };
                pen.fill(
                    fill,
                    Paint {
                        color: INK,
                        alpha: 1.0,
                    },
                );
            }
            let label_y = BODY_TOP + col_h + CPU_LABEL_GAP + CPU_LABEL_SIZE;
            let centre = core_x + core_w / 2.0;
            pen.text(
                (centre, label_y),
                CPU_LABEL_SIZE,
                QUIET,
                Align::Center,
                &format!("C{core}"),
            );
        }
    }

    /// The network graph and its heading.
    fn draw_net(&self, pen: &mut Pen<'_, '_>) {
        let left = 0.0;
        pen.text(
            (left, HEAD_BASELINE),
            HEAD_SIZE,
            1.0,
            Align::Left,
            "NETWORK",
        );
        let latest = self.snapshot.network;
        let slots = [
            ("DN", visualizer::shade(0.0), latest.map(|n| n.down_kbps)),
            ("UP", NET_UP_COLOR, latest.map(|n| n.up_kbps)),
        ];
        let first_slot = left + COLUMN_W - NET_SLOT_W * 2.0;
        for (slot, (name, color, kbps)) in (0_u32..).zip(slots) {
            let x = first_slot + f64::from(slot) * NET_SLOT_W;
            let key_y = HEAD_BASELINE - HEAD_SIZE * 0.35;
            pen.fill(
                Area {
                    x0: x,
                    y0: key_y - WEIGHT / 2.0,
                    x1: x + KEY_W,
                    y1: key_y + WEIGHT / 2.0,
                },
                Paint { color, alpha: 1.0 },
            );
            let value = kbps.map_or_else(|| MISSING.to_owned(), format_rate);
            pen.text(
                (x + KEY_W + KEY_GAP, HEAD_BASELINE),
                NET_META_SIZE,
                QUIET,
                Align::Left,
                &format!("{name} {value}"),
            );
        }
        self.draw_net_graph(pen);
    }

    /// The network graph's plot: download (shaded green to red by speed)
    /// and upload (blue), as rounded lines over filled areas.
    fn draw_net_graph(&self, pen: &mut Pen<'_, '_>) {
        let left = 0.0;
        let top = BODY_TOP + NET_TOP;
        let base = top + NET_H;
        let right = left + COLUMN_W;
        pen.dashed((left, right), top + NET_H / 2.0, GRID);
        pen.fill(
            Area {
                x0: left,
                y0: base - WEIGHT / 2.0,
                x1: right,
                y1: base + WEIGHT / 2.0,
            },
            Paint {
                color: INK,
                alpha: TRACK,
            },
        );

        let peak = self.net_peak_shown();
        // Newest sample at the right edge; the line grows in from there.
        // While sliding, everything sits up to a step to the right and the
        // line is cut at the graph's edges.
        let span = count_to_f64(self.net_span().saturating_sub(1));
        let step = COLUMN_W / span;
        let slide = (1.0 - self.net_phase.clamp(0.0, 1.0)) * step;
        let newest = count_to_f64(self.net.len().saturating_sub(1));
        let point = |i: u32, kbps: f64| {
            let x = right - (newest - f64::from(i)) * step + slide;
            let height = (kbps / peak).clamp(0.0, 1.0);
            (x, top + NET_H * (1.0 - height))
        };
        let steps = curve_steps(step);
        let line = |pick: fn(&Network) -> f64| -> Vec<(f64, f64)> {
            let points: Vec<(f64, f64)> = (0_u32..)
                .zip(&self.net)
                .map(|(i, n)| point(i, pick(n)))
                .collect();
            clip_x(&round_line(&points, steps), left, right)
        };
        let down = line(|n| n.down_kbps);
        let up = line(|n| n.up_kbps);
        // Download is shaded by speed: green when idle, red at the
        // adapter's (or plan's) maximum; relative to the graph's height
        // when neither is known.
        let max = self.net_max_kbps();
        let color_at = |y: f64| {
            let height = ((base - y) / NET_H).clamp(0.0, 1.0);
            let share = max.map_or(height, |max| height * peak / max);
            visualizer::shade(share)
        };
        pen.area_under_shaded(&down, base, NET_DOWN_FILL, color_at);
        pen.area_under(
            &up,
            base,
            Paint {
                color: NET_UP_COLOR,
                alpha: NET_UP_FILL,
            },
        );
        pen.line_shaded(&down, color_at);
        pen.line(
            &up,
            Paint {
                color: NET_UP_COLOR,
                alpha: 1.0,
            },
        );

        pen.text(
            (right, top + NET_TEXT - 1.0),
            NET_TEXT,
            QUIET,
            Align::Right,
            &format_rate(peak),
        );
        let labels_y = BODY_TOP + NET_GRAPH_H - 3.0;
        let since = format!("-{}S", self.net_seconds());
        pen.text((left, labels_y), NET_TEXT, QUIET, Align::Left, &since);
        pen.text((right, labels_y), NET_TEXT, QUIET, Align::Right, "NOW");
    }

    /// One ring chart per GPU, two per row.
    fn draw_gpus(&self, pen: &mut Pen<'_, '_>) {
        let levels = self.shown();
        for (spot, index) in self.shown_gpus().into_iter().enumerate() {
            if let Some(gpu) = self.snapshot.gpus.get(index) {
                let rings = gpu_rings(&levels.rings, index);
                draw_ring_chart(pen, gpu_grid_spot(spot), gpu, rings);
            }
        }
    }

    /// RAM use and SSD temperatures, on one line.
    fn draw_info(&self, pen: &mut Pen<'_, '_>) {
        let memory = self
            .snapshot
            .memory
            .filter(|m| self.parts.ram && m.total_bytes > 0);
        if let Some(memory) = memory {
            let used = u64_to_f64(memory.used_bytes);
            let total = u64_to_f64(memory.total_bytes);
            let text = format!(
                "RAM {:.0}%  {:.1}/{:.1} GB",
                used / total * PERCENT_MAX,
                used / BYTES_PER_GB,
                total / BYTES_PER_GB
            );
            let at = (0.0, HEAD_BASELINE);
            pen.text(at, HEAD_SIZE, QUIET, Align::Left, &text);
            draw_ram_bar(pen, self.shown().ram, memory.modules);
        }
        let disks: Vec<String> = self
            .snapshot
            .disks
            .iter()
            .map(|disk| format!("SSD{} {} °C", disk.disk, disk.temp_c))
            .collect();
        if self.parts.ssd {
            let text = disks.join("     ");
            // In the right column beside the RAM, or on the left alone.
            let x = if self.parts.ram {
                COLUMN_W + COLUMN_GAP
            } else {
                0.0
            };
            pen.text((x, HEAD_BASELINE), HEAD_SIZE, QUIET, Align::Left, &text);
        }
    }
}

/// Draws one GPU's rings, names and legend with its top-left at
/// `origin`.
/// One GPU's chart: rings filled to `rings` (its five fractions, in
/// ring order; missing ones draw empty), its name and the legend.
fn draw_ring_chart(
    pen: &mut Pen<'_, '_>,
    (left, top): (f64, f64),
    gpu: &Gpu,
    rings: &[f64],
) {
    let to_units = RING_SIZE / RING_VIEW;
    let centre = (left + RING_SIZE / 2.0, top + RING_SIZE / 2.0);
    for ((index, ring), fraction) in (0_u32..)
        .zip(Ring::ALL)
        .zip(rings.iter().copied().chain(std::iter::repeat(0.0)))
    {
        let radius = (RING_OUTER_R - f64::from(index) * RING_GAP) * to_units;
        pen.arc(
            centre,
            radius,
            fraction,
            Paint {
                color: ring.color(),
                alpha: 1.0,
            },
        );
    }

    let (series, model) = split_gpu_name(&gpu.name);
    pen.text(
        (left, top + MODEL_BASELINE),
        MODEL_SIZE,
        1.0,
        Align::Left,
        &model,
    );
    pen.text(
        (left, top + SERIES_BASELINE),
        SERIES_SIZE,
        QUIET,
        Align::Left,
        &series,
    );

    let legend_x = left + RING_SIZE + LEGEND_GAP;
    let rows = count_to_f64(Ring::ALL.len());
    let legend_top = centre.1 - rows * LEGEND_ROW_H / 2.0;
    for (index, ring) in (0_u32..).zip(Ring::ALL) {
        let row_top = legend_top + f64::from(index) * LEGEND_ROW_H;
        let middle = row_top + LEGEND_ROW_H / 2.0;
        pen.fill(
            Area {
                x0: legend_x,
                y0: middle - WEIGHT / 2.0,
                x1: legend_x + KEY_W,
                y1: middle + WEIGHT / 2.0,
            },
            Paint {
                color: ring.color(),
                alpha: 1.0,
            },
        );
        let baseline = middle + HEAD_SIZE * 0.35;
        let label_x = legend_x + KEY_W + LEGEND_COL_GAP;
        pen.text(
            (label_x, baseline),
            HEAD_SIZE,
            QUIET,
            Align::Left,
            ring.label(),
        );
        let value_x = label_x + LEGEND_LABEL_W + LEGEND_COL_GAP;
        let (_, text) = ring_reading(gpu, ring);
        pen.text((value_x, baseline), HEAD_SIZE, 1.0, Align::Left, &text);
    }
}

/// One ring's fill (0.0 to 1.0, `None` if unknown) and legend text.
fn ring_reading(gpu: &Gpu, ring: Ring) -> (Option<f64>, String) {
    let percent = |v: Option<f64>| v.map(|v| (v / PERCENT_MAX).clamp(0.0, 1.0));
    let whole = |v: Option<f64>, unit: &str| {
        v.map_or_else(|| MISSING.to_owned(), |v| format!("{v:.0} {unit}"))
    };
    match ring {
        Ring::Load => (percent(gpu.usage), whole(gpu.usage, "%")),
        Ring::Vram => {
            let used = gpu.vram_used_bytes.map(u64_to_f64);
            let total =
                gpu.vram_total_bytes.map(u64_to_f64).filter(|t| *t > 0.0);
            let fraction =
                used.zip(total).map(|(u, t)| (u / t).clamp(0.0, 1.0));
            let text = match (used, total) {
                (Some(u), Some(t)) => {
                    format!(
                        "{:.1}/{:.1} GB",
                        u / BYTES_PER_GB,
                        t / BYTES_PER_GB
                    )
                }
                _ => MISSING.to_owned(),
            };
            (fraction, text)
        }
        Ring::Temp => {
            let max = gpu.temp_max_c.unwrap_or(TEMP_MAX_FALLBACK);
            let fraction = gpu
                .temp_c
                .map(|t| ((t - TEMP_MIN) / (max - TEMP_MIN)).clamp(0.0, 1.0));
            (fraction, whole(gpu.temp_c, "°C"))
        }
        Ring::Power => {
            (percent(gpu.power_percent), whole(gpu.power_percent, "%"))
        }
        Ring::Fan => {
            let rpm = gpu.fan_rpm.map(f64::from);
            let max = gpu.fan_max_rpm.map_or(FAN_MAX_FALLBACK, f64::from);
            let fraction = rpm.map(|r| (r / max).clamp(0.0, 1.0));
            (fraction, whole(rpm, "rpm"))
        }
    }
}

/// `NVIDIA GeForce GTX 1070 Ti` → (`GTX`, `1070 Ti`). Names without a
/// known series come back as ("", the name minus its vendor).
pub fn split_gpu_name(name: &str) -> (String, String) {
    const VENDORS: [&str; 5] = [
        "NVIDIA GeForce ",
        "NVIDIA ",
        "AMD Radeon ",
        "AMD ",
        "Intel(R) ",
    ];
    const SERIES: [&str; 5] = ["RTX", "GTX", "GT", "RX", "Arc"];
    let short = VENDORS
        .iter()
        .find_map(|vendor| strip_prefix_ignore_case(name, vendor))
        .unwrap_or(name);
    if let Some((first, rest)) = short.split_once(' ')
        && SERIES.iter().any(|s| s.eq_ignore_ascii_case(first))
        && !rest.trim().is_empty()
    {
        return (first.to_ascii_uppercase(), rest.trim().to_owned());
    }
    (String::new(), short.to_owned())
}

/// `text` without `prefix`, compared ignoring ASCII case.
fn strip_prefix_ignore_case<'a>(
    text: &'a str,
    prefix: &str,
) -> Option<&'a str> {
    let head = text.get(..prefix.len())?;
    head.eq_ignore_ascii_case(prefix)
        .then(|| text.get(prefix.len()..))
        .flatten()
}

/// A network rate as the wallpaper shows it: "<1 MBPS" under 1 Mbps,
/// one decimal under 100 Mbps, whole numbers above.
pub fn format_rate(kbps: f64) -> String {
    let mbps = kbps / KBPS_PER_MBPS;
    if mbps < 1.0 {
        "<1 MBPS".to_owned()
    } else if mbps < WHOLE_MBPS_FROM {
        format!("{mbps:.1} MBPS")
    } else {
        format!("{mbps:.0} MBPS")
    }
}

/// Width of the panel: two widget columns and the gap.
const PANEL_W: f64 = COLUMN_W * 2.0 + COLUMN_GAP;

/// How to group `threads` columns into cores: Windows' layout when it
/// fits the readings, else pairs (or singles for an odd count).
fn core_layout(cores: &[Vec<usize>], threads: usize) -> Vec<Vec<usize>> {
    let fits = !cores.is_empty()
        && cores.iter().flatten().all(|&index| index < threads);
    if fits {
        return cores.to_vec();
    }
    let per_core = if threads.is_multiple_of(THREADS_PER_CORE) {
        THREADS_PER_CORE
    } else {
        1
    };
    (0..threads)
        .collect::<Vec<_>>()
        .chunks(per_core)
        .map(<[usize]>::to_vec)
        .collect()
}

/// Points the rounded graph line adds per reading when readings are
/// `step` units apart: enough for a smooth curve, at most
/// [`NET_CURVE_MAX_STEPS`].
fn curve_steps(step: f64) -> u32 {
    let mut steps = 1;
    while steps < NET_CURVE_MAX_STEPS
        && step / f64::from(steps) > NET_CURVE_STEP
    {
        steps = steps.saturating_add(1);
    }
    steps
}

/// A rounded line through `points` (sorted left to right), with `steps`
/// points per segment. The curve is monotone between neighbouring
/// points (Fritsch-Carlson tangents), so it never overshoots a peak or
/// dips below the baseline.
fn round_line(points: &[(f64, f64)], steps: u32) -> Vec<(f64, f64)> {
    let slopes: Vec<f64> = points
        .windows(2)
        .map(|pair| match pair {
            [a, b] if b.0 - a.0 > f64::EPSILON => (b.1 - a.1) / (b.0 - a.0),
            _ => 0.0,
        })
        .collect();
    let (Some(&first), Some(&last)) = (slopes.first(), slopes.last()) else {
        return points.to_vec();
    };
    // A tangent per point: the harmonic mean of the slopes either side,
    // or flat at a peak or valley.
    let tangents: Vec<f64> = std::iter::once(first)
        .chain(slopes.windows(2).map(|pair| match pair {
            [a, b] if a * b > 0.0 => 2.0 * a * b / (a + b),
            _ => 0.0,
        }))
        .chain(std::iter::once(last))
        .collect();
    let per_segment = usize::try_from(steps).unwrap_or(1);
    let mut curve =
        Vec::with_capacity(points.len().saturating_mul(per_segment));
    curve.extend(points.first());
    for (pair, tangent) in points.windows(2).zip(tangents.windows(2)) {
        if let ([a, b], [ta, tb]) = (pair, tangent) {
            let h = b.0 - a.0;
            for i in 1..=steps {
                let t = f64::from(i) / f64::from(steps);
                let (t2, t3) = (t * t, t * t * t);
                let y = (2.0 * t3 - 3.0 * t2 + 1.0) * a.1
                    + (t3 - 2.0 * t2 + t) * h * ta
                    + (3.0 * t2 - 2.0 * t3) * b.1
                    + (t3 - t2) * h * tb;
                curve.push((a.0 + h * t, y));
            }
        }
    }
    curve
}

/// The part of `points` (sorted left to right) between `x0` and `x1`,
/// with points added where the line crosses each edge.
fn clip_x(points: &[(f64, f64)], x0: f64, x1: f64) -> Vec<(f64, f64)> {
    let at = |a: (f64, f64), b: (f64, f64), x: f64| {
        let span = b.0 - a.0;
        let t = if span.abs() > f64::EPSILON {
            (x - a.0) / span
        } else {
            0.0
        };
        (x, a.1 + (b.1 - a.1) * t)
    };
    let mut out = Vec::with_capacity(points.len());
    for pair in points.windows(2) {
        let [a, b] = pair else {
            continue;
        };
        if b.0 < x0 || a.0 > x1 {
            continue;
        }
        if out.is_empty() {
            out.push(if a.0 < x0 { at(*a, *b, x0) } else { *a });
        }
        out.push(if b.0 > x1 { at(*a, *b, x1) } else { *b });
    }
    if points.len() == 1 {
        out.extend(points.iter().filter(|p| p.0 >= x0 && p.0 <= x1));
    }
    out
}

/// The network graph's plot (the line and its scale label), in the
/// network widget's units.
fn net_plot_area() -> Area {
    Area {
        x0: -MOVING_MARGIN,
        y0: BODY_TOP,
        x1: COLUMN_W + MOVING_MARGIN,
        y1: BODY_TOP + NET_TOP + NET_H + MOVING_MARGIN,
    }
}

/// Where GPU `index`'s chart goes (top-left of its rings) when the
/// charts are laid out two per row.
fn gpu_grid_spot(index: usize) -> (f64, f64) {
    let row = count_to_f64(index / RINGS_PER_ROW);
    let column = count_to_f64(index % RINGS_PER_ROW);
    (
        column * (COLUMN_W + COLUMN_GAP),
        row * (GPU_CHART_H + ROW_GAP) + RING_NAME_ROOM,
    )
}

/// GPU `index`'s five ring fractions out of everyone's.
fn gpu_rings(all: &[f64], index: usize) -> &[f64] {
    let count = Ring::ALL.len();
    let start = index.saturating_mul(count);
    all.get(start..start.saturating_add(count))
        .unwrap_or_default()
}

/// The rings' square for a chart whose rings start at `(left, top)`,
/// with a unit to spare for smoothing.
fn ring_area((left, top): (f64, f64)) -> Area {
    Area {
        x0: left - MOVING_MARGIN,
        y0: top - MOVING_MARGIN,
        x1: left + RING_SIZE + MOVING_MARGIN,
        y1: top + RING_SIZE + MOVING_MARGIN,
    }
}

/// The CPU widget's columns.
fn cpu_columns_area() -> Area {
    Area {
        x0: -MOVING_MARGIN,
        y0: BODY_TOP - MOVING_MARGIN,
        x1: COLUMN_W + MOVING_MARGIN,
        y1: BODY_TOP + cpu_column_height() + MOVING_MARGIN,
    }
}

/// The whole CPU widget (heading, columns, labels).
fn cpu_block_area() -> Area {
    Area {
        x0: -MOVING_MARGIN,
        y0: -MOVING_MARGIN,
        x1: COLUMN_W + MOVING_MARGIN,
        y1: BODY_TOP + CPU_COLS_H + MOVING_MARGIN,
    }
}

/// The whole network widget (heading, graph, labels).
fn net_block_area() -> Area {
    Area {
        x0: -MOVING_MARGIN,
        y0: -MOVING_MARGIN,
        x1: COLUMN_W + MOVING_MARGIN,
        y1: BODY_TOP + NET_GRAPH_H + MOVING_MARGIN,
    }
}

/// The RAM bar, in the info row.
fn ram_bar_area() -> Area {
    Area {
        x0: -MOVING_MARGIN,
        y0: RAM_BAR_TOP - MOVING_MARGIN,
        x1: COLUMN_W + MOVING_MARGIN,
        y1: RAM_BAR_TOP + RAM_BAR_H + MOVING_MARGIN,
    }
}

/// `area` moved right by `dx`.
fn shift_x(area: Area, dx: f64) -> Area {
    Area {
        x0: area.x0 + dx,
        x1: area.x1 + dx,
        ..area
    }
}

/// `area` moved down by `dy`.
fn shift(area: Area, dy: f64) -> Area {
    Area {
        y0: area.y0 + dy,
        y1: area.y1 + dy,
        ..area
    }
}

/// Height of a CPU column (the area above the core labels).
fn cpu_column_height() -> f64 {
    CPU_COLS_H - CPU_LABEL_GAP - CPU_LABEL_SIZE * 1.2
}

/// Size of `count` GPU charts laid out two per row.
fn gpus_size(count: usize) -> (f64, f64) {
    if count == 0 {
        return (0.0, 0.0);
    }
    let columns = count_to_f64(count.min(RINGS_PER_ROW));
    let rows = count_to_f64(count.div_ceil(RINGS_PER_ROW));
    (
        columns * COLUMN_W + (columns - 1.0) * COLUMN_GAP,
        rows * GPU_CHART_H + (rows - 1.0) * ROW_GAP,
    )
}

/// The RAM bar under the RAM text, a column wide: a dim track filled to
/// `fraction` (0 to 1), split into a section per stick in the
/// wallpaper's colours (one section when the count is unknown).
fn draw_ram_bar(pen: &mut Pen<'_, '_>, fraction: f64, modules: usize) {
    let sections = modules.max(1);
    let count = count_to_f64(sections);
    let section_w = (COLUMN_W - RAM_BAR_GAP * (count - 1.0)) / count;
    let filled = fraction.clamp(0.0, 1.0) * COLUMN_W;
    let colors = STICK_COLORS.iter().cycle();
    for (index, color) in (0_u32..).zip(colors).take(sections) {
        let x0 = f64::from(index) * (section_w + RAM_BAR_GAP);
        let area = Area {
            x0,
            y0: RAM_BAR_TOP,
            x1: x0 + section_w,
            y1: RAM_BAR_TOP + RAM_BAR_H,
        };
        pen.fill(
            area,
            Paint {
                color: INK,
                alpha: TRACK,
            },
        );
        let fill_to = filled.min(area.x1);
        if fill_to > x0 {
            let fill = Area {
                x1: fill_to,
                ..area
            };
            pen.fill(
                fill,
                Paint {
                    color: *color,
                    alpha: 1.0,
                },
            );
        }
    }
}

/// A small count as `f64`.
fn count_to_f64(count: usize) -> f64 {
    u32::try_from(count).map_or(f64::MAX, f64::from)
}

/// Draws in design units onto a canvas at a scale, collecting labels.
struct Pen<'a, 'b> {
    /// Where shapes go.
    canvas: &'a mut Canvas<'b>,
    /// Pixels per unit.
    scale: f64,
    /// The background, for mixing text opacity.
    background: Rgb,
    /// Where (0, 0) is, in units: lets each widget draw in its own
    /// coordinates inside the panel.
    origin: (f64, f64),
    /// Text collected for GDI.
    labels: Vec<Label>,
    /// Clickable areas collected.
    hits: Vec<Hit>,
}

impl Pen<'_, '_> {
    /// An area from units to pixels.
    fn px(&self, area: Area) -> Area {
        let (ox, oy) = self.origin;
        Area {
            x0: (area.x0 + ox) * self.scale,
            y0: (area.y0 + oy) * self.scale,
            x1: (area.x1 + ox) * self.scale,
            y1: (area.y1 + oy) * self.scale,
        }
    }

    /// A point from units to pixels.
    fn point(&self, (x, y): (f64, f64)) -> (f64, f64) {
        let (ox, oy) = self.origin;
        ((x + ox) * self.scale, (y + oy) * self.scale)
    }

    /// Points from units to pixels.
    fn points(&self, points: &[(f64, f64)]) -> Vec<(f64, f64)> {
        points.iter().map(|&p| self.point(p)).collect()
    }

    /// The line weight in pixels, never thinner than one pixel.
    fn weight(&self) -> f64 {
        (WEIGHT * self.scale).max(1.0)
    }

    /// Fills an area.
    fn fill(&mut self, area: Area, paint: Paint) {
        let area = self.px(area);
        self.canvas.fill_rect(area, paint);
    }

    /// Fills an area with a colour that changes down it: `color_at`
    /// gets each row's height in units.
    fn fill_shaded(&mut self, area: Area, color_at: impl Fn(f64) -> Rgb) {
        let px = self.px(area);
        let (scale, oy) = (self.scale, self.origin.1);
        self.canvas.fill_rect_rows(px, |y| Paint {
            color: color_at(y / scale - oy),
            alpha: 1.0,
        });
    }

    /// Outlines an area in ink at `alpha`.
    fn stroke(&mut self, area: Area, alpha: f64) {
        let (area, weight) = (self.px(area), self.weight());
        self.canvas
            .stroke_rect(area, weight, Paint { color: INK, alpha });
    }

    /// A dashed horizontal ink line at `alpha`.
    fn dashed(&mut self, (x0, x1): (f64, f64), y: f64, alpha: f64) {
        let s = self.scale;
        let weight = self.weight();
        let (start, y) = self.point((x0, y));
        let (end, _) = self.point((x1, 0.0));
        self.canvas.dashed_hline(
            (start, end),
            y,
            weight,
            (NET_DASH.0 * s, NET_DASH.1 * s),
            Paint { color: INK, alpha },
        );
    }

    /// A graph's line through points sorted left to right.
    fn line(&mut self, points: &[(f64, f64)], paint: Paint) {
        let (points, weight) = (self.points(points), self.weight());
        self.canvas.graph_line(&points, weight, paint);
    }

    /// A graph's line whose colour depends on height: `color_at` gets
    /// each row's height in units.
    fn line_shaded(
        &mut self,
        points: &[(f64, f64)],
        color_at: impl Fn(f64) -> Rgb,
    ) {
        let (points, weight) = (self.points(points), self.weight());
        let (scale, oy) = (self.scale, self.origin.1);
        self.canvas.graph_line_rows(&points, weight, |y| Paint {
            color: color_at(y / scale - oy),
            alpha: 1.0,
        });
    }

    /// The area under a line, in one paint.
    fn area_under(&mut self, points: &[(f64, f64)], base: f64, paint: Paint) {
        let points = self.points(points);
        let (_, base) = self.point((0.0, base));
        self.canvas.fill_under_rows(&points, base, |_| paint);
    }

    /// The area under a line, shaded by height like
    /// [`Pen::line_shaded`], at `alpha`.
    fn area_under_shaded(
        &mut self,
        points: &[(f64, f64)],
        base: f64,
        alpha: f64,
        color_at: impl Fn(f64) -> Rgb,
    ) {
        let points = self.points(points);
        let (_, base) = self.point((0.0, base));
        let (scale, oy) = (self.scale, self.origin.1);
        self.canvas.fill_under_rows(&points, base, |y| Paint {
            color: color_at(y / scale - oy),
            alpha,
        });
    }

    /// A ring arc.
    fn arc(
        &mut self,
        (cx, cy): (f64, f64),
        radius: f64,
        fraction: f64,
        paint: Paint,
    ) {
        let weight = self.weight();
        let centre = self.point((cx, cy));
        self.canvas
            .arc(centre, radius * self.scale, weight, fraction, paint);
    }

    /// Makes an area (in units) clickable.
    fn hit(&mut self, area: Area, action: Action) {
        let area = self.px(area);
        self.hits.push(Hit { area, action });
    }

    /// A filled circle.
    fn circle(
        &mut self,
        centre: (f64, f64),
        radius: f64,
        color: Rgb,
        alpha: f64,
    ) {
        let centre = self.point(centre);
        let radius = radius * self.scale;
        // A ring as wide as the radius, centred halfway out, is a disc.
        self.canvas.arc(
            centre,
            radius / 2.0,
            radius,
            1.0,
            Paint { color, alpha },
        );
    }

    /// An image clipped to a circle.
    fn image_circle(
        &mut self,
        centre: (f64, f64),
        radius: f64,
        image: &crate::image::Image,
    ) {
        let centre = self.point(centre);
        self.canvas.image_circle(centre, radius * self.scale, image);
    }

    /// An image clipped to a rounded square.
    fn image_rounded(
        &mut self,
        centre: (f64, f64),
        half: f64,
        corner: f64,
        image: &crate::image::Image,
    ) {
        let centre = self.point(centre);
        let s = self.scale;
        self.canvas
            .image_rounded(centre, half * s, corner * s, image);
    }

    /// A filled rounded square.
    fn rounded(
        &mut self,
        centre: (f64, f64),
        half: f64,
        corner: f64,
        color: Rgb,
        alpha: f64,
    ) {
        let centre = self.point(centre);
        let s = self.scale;
        self.canvas.fill_rounded(
            centre,
            half * s,
            corner * s,
            Paint { color, alpha },
        );
    }

    /// A circle outline `width` units wide.
    fn ring(
        &mut self,
        centre: (f64, f64),
        radius: f64,
        width: f64,
        color: Rgb,
        alpha: f64,
    ) {
        let centre = self.point(centre);
        let width = (width * self.scale).max(1.0);
        self.canvas.arc(
            centre,
            radius * self.scale,
            width,
            1.0,
            Paint { color, alpha },
        );
    }

    /// A line `width` units wide.
    fn line_width(&mut self, points: &[(f64, f64)], width: f64, paint: Paint) {
        let points = self.points(points);
        let width = (width * self.scale).max(1.0);
        self.canvas.polyline(&points, width, paint);
    }

    /// Text in `color` at `alpha`.
    fn text_in(
        &mut self,
        color: Rgb,
        (x, y): (f64, f64),
        size: f64,
        alpha: f64,
        align: Align,
        text: &str,
    ) {
        if text.is_empty() {
            return;
        }
        let (x, y) = self.point((x, y));
        self.labels.push(Label {
            x,
            y,
            size: size * self.scale,
            color: mix(color, self.background, alpha),
            align,
            text: text.to_owned(),
            clip: None,
        });
    }

    /// Left-aligned text in ink at `alpha`, shown only inside `clip`
    /// (units): for text scrolling through a box.
    fn text_clipped(
        &mut self,
        (x, y): (f64, f64),
        size: f64,
        alpha: f64,
        clip: Area,
        text: &str,
    ) {
        if text.is_empty() {
            return;
        }
        let (x, y) = self.point((x, y));
        let clip = self.px(clip);
        self.labels.push(Label {
            x,
            y,
            size: size * self.scale,
            color: mix(INK, self.background, alpha),
            align: Align::Left,
            text: text.to_owned(),
            clip: Some(clip),
        });
    }

    /// Text in ink at `alpha`.
    fn text(
        &mut self,
        (x, y): (f64, f64),
        size: f64,
        alpha: f64,
        align: Align,
        text: &str,
    ) {
        if text.is_empty() {
            return;
        }
        let (x, y) = self.point((x, y));
        self.labels.push(Label {
            x,
            y,
            size: size * self.scale,
            color: mix(INK, self.background, alpha),
            align,
            text: text.to_owned(),
            clip: None,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::{
        Dashboard, Parts, Ring, Widget, format_rate, ring_reading,
        split_gpu_name,
    };
    use crate::canvas::Canvas;
    use crate::color::Rgb;
    use crate::sensors::{Cpu, Gpu, Network, Snapshot};

    fn split(name: &str) -> (String, String) {
        split_gpu_name(name)
    }

    #[test]
    fn splits_gpu_names() {
        assert_eq!(
            split("NVIDIA GeForce GTX 1070 Ti"),
            ("GTX".into(), "1070 Ti".into())
        );
        assert_eq!(
            split("NVIDIA GeForce RTX 2080"),
            ("RTX".into(), "2080".into())
        );
        assert_eq!(
            split("AMD Radeon RX 7900 XTX"),
            ("RX".into(), "7900 XTX".into())
        );
        assert_eq!(split("Intel(R) Arc A770"), ("ARC".into(), "A770".into()));
        assert_eq!(
            split("Intel(R) UHD Graphics"),
            (String::new(), "UHD Graphics".into())
        );
        assert_eq!(split("RTX"), (String::new(), "RTX".into()));
    }

    #[test]
    fn formats_rates() {
        assert_eq!(format_rate(400.0), "<1 MBPS");
        assert_eq!(format_rate(5_830.1), "5.8 MBPS");
        assert_eq!(format_rate(250_000.0), "250 MBPS");
    }

    #[test]
    fn ring_readings() {
        let gpu = Gpu {
            usage: Some(58.5),
            temp_c: Some(51.5),
            temp_max_c: Some(83.0),
            fan_rpm: Some(1650),
            fan_max_rpm: None,
            power_percent: Some(150.0),
            vram_used_bytes: Some(1 << 30),
            vram_total_bytes: Some(8 << 30),
            ..Gpu::default()
        };
        assert_eq!(
            ring_reading(&gpu, Ring::Load),
            (Some(0.585), "58 %".into())
        );
        assert_eq!(ring_reading(&gpu, Ring::Temp), (Some(0.5), "52 °C".into()));
        assert_eq!(
            ring_reading(&gpu, Ring::Fan),
            (Some(0.55), "1650 rpm".into())
        );
        assert_eq!(
            ring_reading(&gpu, Ring::Power),
            (Some(1.0), "150 %".into())
        );
        assert_eq!(
            ring_reading(&gpu, Ring::Vram),
            (Some(0.125), "1.0/8.0 GB".into())
        );
        let unknown = Gpu::default();
        assert_eq!(ring_reading(&unknown, Ring::Load), (None, "--".into()));
    }

    /// Feeds `count` network readings, the `i`th downloading `i` kbit/s.
    fn feed_network(dashboard: &mut Dashboard, count: u32) {
        for i in 0..count {
            dashboard.update(Snapshot {
                network: Some(Network {
                    down_kbps: f64::from(i),
                    up_kbps: 0.0,
                    link_kbps: None,
                }),
                ..Snapshot::default()
            });
        }
    }

    #[test]
    fn keeps_the_graphs_span_of_network_history() {
        let mut dashboard = Dashboard::default();
        // 30 s at four readings a second.
        assert_eq!(dashboard.net_span(), 120);
        feed_network(&mut dashboard, 160);
        // The span, plus one sample to slide in from the left edge.
        assert_eq!(dashboard.net.len(), 121);
        assert_eq!(dashboard.net.front().map(|n| n.down_kbps), Some(39.0));

        dashboard.set_net_seconds(60);
        feed_network(&mut dashboard, 300);
        assert_eq!(dashboard.net.len(), 241);
        dashboard.set_net_seconds(10);
        assert_eq!(dashboard.net.len(), 41);
        assert_eq!(dashboard.net.back().map(|n| n.down_kbps), Some(299.0));
    }

    #[test]
    fn rounded_line_keeps_its_points_without_overshooting() {
        let points = [(0.0, 10.0), (1.0, 10.0), (2.0, 0.0), (3.0, 5.0)];
        let curve = super::round_line(&points, 4);
        assert_eq!(curve.len(), 13);
        // Every fourth point is one of the originals.
        for (i, point) in points.iter().enumerate() {
            let (x, y) = curve[i * 4];
            assert!((x - point.0).abs() < 1e-9 && (y - point.1).abs() < 1e-9);
        }
        // Monotone: nothing above the peak or below the valley, and the
        // flat start stays flat.
        assert!(curve.iter().all(|&(_, y)| (0.0..=10.0).contains(&y)));
        assert!(curve[..5].iter().all(|&(_, y)| (y - 10.0).abs() < 1e-9));
        // In between it curves rather than running straight.
        let (_, quarter) = curve[5];
        assert!((quarter - 7.5).abs() > 0.5);
        assert_eq!(super::round_line(&points[..1], 4), points[..1]);
    }

    #[test]
    fn rounded_line_adds_points_by_spacing() {
        assert_eq!(super::curve_steps(0.2), 1);
        assert_eq!(super::curve_steps(1.6), 4);
        assert_eq!(super::curve_steps(100.0), 8);
    }

    #[test]
    fn cuts_lines_at_the_edges() {
        let points = [(-2.0, 0.0), (2.0, 4.0), (6.0, 0.0), (12.0, 6.0)];
        let cut = super::clip_x(&points, 0.0, 10.0);
        assert_eq!(cut, vec![(0.0, 2.0), (2.0, 4.0), (6.0, 0.0), (10.0, 4.0)]);
        assert_eq!(super::clip_x(&points, 20.0, 30.0), Vec::new());
    }

    #[test]
    fn sizes_each_widget() {
        let mut dashboard = Dashboard::default();
        dashboard.update(Snapshot {
            gpus: vec![Gpu::default(); 3],
            ..Snapshot::default()
        });
        assert_eq!(dashboard.size(Widget::Cpu), (195.0, 98.0));
        assert_eq!(dashboard.size(Widget::Network), (195.0, 109.0));
        assert_eq!(dashboard.size(Widget::Gpu(2)), (195.0, 90.0));
        assert_eq!(dashboard.size(Widget::Gpu(3)), (0.0, 0.0), "no 4th GPU");
        // Three GPUs: two columns, two rows.
        assert_eq!(dashboard.size(Widget::Gpus), (432.5, 90.0 * 2.0 + 16.0));
        let (panel_w, _) = dashboard.size(Widget::Panel);
        assert!((panel_w - 432.5).abs() < f64::EPSILON);
    }

    #[test]
    fn grows_a_row_per_two_gpus() {
        let mut dashboard = Dashboard::default();
        let (_, empty) = dashboard.size(Widget::Panel);
        dashboard.update(Snapshot {
            gpus: vec![Gpu::default(), Gpu::default()],
            ..Snapshot::default()
        });
        let (_, two) = dashboard.size(Widget::Panel);
        dashboard.update(Snapshot {
            gpus: vec![Gpu::default(); 3],
            ..Snapshot::default()
        });
        let (_, three) = dashboard.size(Widget::Panel);
        assert!(two > empty);
        assert!(three > two);
    }

    #[test]
    fn draws_without_panicking_and_labels_everything() {
        let mut dashboard = Dashboard::default();
        dashboard.update(Snapshot {
            cpu: Cpu {
                total: Some(10.0),
                per_core: vec![10.0; 12],
                clock_ghz: Some(4.3),
                cores: Vec::new(),
            },
            gpus: vec![Gpu {
                name: "NVIDIA GeForce RTX 2080".into(),
                ..Gpu::default()
            }],
            ..Snapshot::default()
        });
        // 1440p scale; the widgets are about 623 x 380 pixels there.
        let scale = 1.44;
        let (w, h) = dashboard.size(Widget::Panel);
        assert!(w * scale < 700.0 && h * scale < 450.0);
        let mut buffer = vec![0_u32; 700 * 450];
        let labels = {
            let mut canvas = Canvas::new(700, 450, &mut buffer).unwrap();
            dashboard.draw(Widget::Panel, &mut canvas, scale, Rgb::BLACK)
        };
        let texts: Vec<&str> =
            labels.labels.iter().map(|l| l.text.as_str()).collect();
        assert!(texts.contains(&"CPU THREADS"));
        assert!(texts.contains(&"4.30 GHZ · 6C / 12T"));
        assert!(texts.contains(&"C6"));
        assert!(texts.contains(&"2080"));
        assert!(texts.contains(&"RTX"));
        assert!(texts.contains(&"NOW"));
        assert!(buffer.iter().any(|&p| p != 0), "something drawn");
    }

    #[test]
    fn hidden_parts_leave_no_gap() {
        let mut dashboard = Dashboard::default();
        let gpu = |name: &str| Gpu {
            name: name.into(),
            ..Gpu::default()
        };
        dashboard.update(Snapshot {
            gpus: vec![gpu("GTX 1070 Ti"), gpu("RTX 2080")],
            ..Snapshot::default()
        });
        let (_, full) = dashboard.size(Widget::Panel);
        dashboard.set_parts(Parts {
            cpu: false,
            network: false,
            gpus: Some(vec![1]),
            ..Parts::default()
        });
        let (_, trimmed) = dashboard.size(Widget::Panel);
        assert!(trimmed < full / 2.0, "{trimmed} vs {full}");
        let mut buffer = vec![0_u32; 700 * 450];
        let drawing = {
            let mut canvas = Canvas::new(700, 450, &mut buffer).unwrap();
            dashboard.draw(Widget::Panel, &mut canvas, 1.44, Rgb::BLACK)
        };
        let texts: Vec<&str> =
            drawing.labels.iter().map(|l| l.text.as_str()).collect();
        assert!(texts.contains(&"2080"));
        assert!(!texts.contains(&"1070 Ti"));
        assert!(!texts.contains(&"CPU THREADS"));
        assert!(!texts.contains(&"NETWORK"));
    }
}
