//! The widgets: CPU thread columns, the network graph and the GPU rings,
//! redrawn natively from the user's wallpaper designs (`cpu-cores.js`,
//! `net-graph.js`, `gpu-rings.js` and their CSS).
//!
//! Sizes are in design units: 1 unit = 0.1% of the monitor's height (the
//! wallpaper's 0.1vh), which is also the line weight. Shapes go to a
//! [`Canvas`]; text comes back as [`Label`]s for GDI to draw on top.

mod discord;
mod motion;

pub use discord::{Action, Demo};

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
/// Opacity of the fill under the download line.
const AREA: f64 = 0.08;
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

/// Network: samples kept (a minute at two a second).
pub const NET_SPAN: usize = 120;
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
}

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
    /// The last [`NET_SPAN`] network readings, oldest first.
    net: VecDeque<Network>,
    /// The Discord widget's state.
    discord: discord::DiscordUi,
    /// The bars' and rings' glide to the latest reading.
    motion: motion::Motion,
}

impl Dashboard {
    /// Takes in a new reading.
    pub fn update(&mut self, snapshot: Snapshot) {
        let before = self.shown();
        if let Some(network) = snapshot.network {
            if self.net.len() >= NET_SPAN {
                let _oldest = self.net.pop_front();
            }
            self.net.push_back(network);
        }
        self.snapshot = snapshot;
        if before == self.target() {
            self.motion.settle();
        } else {
            self.motion.restart(before);
        }
    }

    /// Moves the bars and rings on by `elapsed`. Returns whether they're
    /// still moving (repaint the [`Dashboard::moving_areas`] until not).
    pub fn advance(&mut self, elapsed: Duration) -> bool {
        self.motion.advance(elapsed)
    }

    /// Whether the bars and rings are gliding to a new reading.
    pub fn is_moving(&self) -> bool {
        self.motion.is_moving()
    }

    /// Jumps straight to the latest reading (for `--preview`).
    pub fn settle(&mut self) {
        self.motion.settle();
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
        self.motion.shown(&self.target())
    }

    /// The parts of `widget` a quick reading (CPU and network only)
    /// changes, in its own units.
    pub fn quick_areas(widget: Widget) -> Vec<Area> {
        let (cpu, net) = (cpu_block_area(), net_block_area());
        match widget {
            Widget::Panel => vec![
                cpu,
                Area {
                    x0: net.x0 + COLUMN_W + COLUMN_GAP,
                    x1: net.x1 + COLUMN_W + COLUMN_GAP,
                    ..net
                },
            ],
            Widget::Cpu => vec![cpu],
            Widget::Network => vec![net],
            Widget::Gpus | Widget::Gpu(_) | Widget::Info | Widget::Discord => {
                Vec::new()
            }
        }
    }

    /// Where `widget` has parts that glide, in its own units: the CPU
    /// columns, the GPU rings and the RAM bar. Repainting just these is
    /// enough while [`Dashboard::is_moving`].
    pub fn moving_areas(&self, widget: Widget) -> Vec<Area> {
        let gpus = self.snapshot.gpus.len();
        let mut areas = Vec::new();
        match widget {
            Widget::Panel => {
                areas.push(cpu_columns_area());
                let top = Self::panel_gpus_top();
                areas.extend(
                    (0..gpus).map(|i| shift(ring_area(gpu_grid_spot(i)), top)),
                );
                areas.push(shift(ram_bar_area(), self.panel_info_top()));
            }
            Widget::Cpu => areas.push(cpu_columns_area()),
            Widget::Network | Widget::Discord => {}
            Widget::Gpus => {
                areas.extend((0..gpus).map(|i| ring_area(gpu_grid_spot(i))));
            }
            Widget::Gpu(index) => {
                if index < gpus {
                    areas.push(ring_area((0.0, RING_NAME_ROOM)));
                }
            }
            Widget::Info => areas.push(ram_bar_area()),
        }
        areas
    }

    /// A widget's size in design units. (0, 0) when it has nothing to
    /// show, such as a GPU that isn't there.
    pub fn size(&self, widget: Widget) -> (f64, f64) {
        match widget {
            Widget::Panel => (PANEL_W, self.panel_info_top() + INFO_H),
            Widget::Cpu => (COLUMN_W, BODY_TOP + CPU_COLS_H),
            Widget::Network => (COLUMN_W, BODY_TOP + NET_GRAPH_H),
            Widget::Gpus => gpus_size(self.snapshot.gpus.len()),
            Widget::Gpu(index) => {
                if index < self.snapshot.gpus.len() {
                    (COLUMN_W, GPU_CHART_H)
                } else {
                    (0.0, 0.0)
                }
            }
            Widget::Info => (PANEL_W, INFO_H),
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
        self.draw_cpu(pen);
        pen.origin = (COLUMN_W + COLUMN_GAP, 0.0);
        self.draw_net(pen);
        pen.origin = (0.0, Self::panel_gpus_top());
        self.draw_gpus(pen);
        pen.origin = (0.0, self.panel_info_top());
        self.draw_info(pen);
        pen.origin = (0.0, 0.0);
    }

    /// Where the GPU charts start in the panel.
    fn panel_gpus_top() -> f64 {
        BODY_TOP + NET_GRAPH_H + ROW_GAP
    }

    /// Where the RAM/SSD line starts in the panel.
    fn panel_info_top(&self) -> f64 {
        let (_, gpus_h) = gpus_size(self.snapshot.gpus.len());
        let gap = if gpus_h > 0.0 { ROW_GAP } else { 0.0 };
        Self::panel_gpus_top() + gpus_h + gap
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
        let per_core = if threads.is_multiple_of(THREADS_PER_CORE) {
            THREADS_PER_CORE
        } else {
            1
        };
        let cores = cpu.per_core.chunks(per_core).count();
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
        let per_core_n = count_to_f64(per_core);
        let col_w = (core_w - CPU_PAIR_GAP * (per_core_n - 1.0)) / per_core_n;
        let col_h = cpu_column_height();

        for (core, loads) in (1_u32..).zip(levels.cores.chunks(per_core)) {
            let core_x = (f64::from(core) - 1.0) * (core_w + CPU_CORE_GAP);
            for (thread, load) in (0_u32..).zip(loads) {
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
            ("DN", INK, latest.map(|n| n.down_kbps)),
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

    /// The network graph's plot: the last minute of download (white,
    /// faint fill) and upload (blue).
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

        let peak = self
            .net
            .iter()
            .flat_map(|n| [n.down_kbps, n.up_kbps])
            .fold(NET_MIN_PEAK_KBPS, f64::max);
        // Newest sample at the right edge; the line grows in from there.
        let offset = count_to_f64(NET_SPAN.saturating_sub(self.net.len()));
        let step = COLUMN_W / count_to_f64(NET_SPAN.saturating_sub(1));
        let point = |i: u32, kbps: f64| {
            let x = left + (f64::from(i) + offset) * step;
            (x, top + NET_H * (1.0 - kbps / peak))
        };
        let down: Vec<(f64, f64)> = (0_u32..)
            .zip(&self.net)
            .map(|(i, n)| point(i, n.down_kbps))
            .collect();
        let up: Vec<(f64, f64)> = (0_u32..)
            .zip(&self.net)
            .map(|(i, n)| point(i, n.up_kbps))
            .collect();
        pen.area_under(
            &down,
            base,
            Paint {
                color: INK,
                alpha: AREA,
            },
        );
        pen.line(
            &down,
            Paint {
                color: INK,
                alpha: 1.0,
            },
        );
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
        pen.text((left, labels_y), NET_TEXT, QUIET, Align::Left, "-60S");
        pen.text((right, labels_y), NET_TEXT, QUIET, Align::Right, "NOW");
    }

    /// One ring chart per GPU, two per row.
    fn draw_gpus(&self, pen: &mut Pen<'_, '_>) {
        let levels = self.shown();
        for (index, gpu) in self.snapshot.gpus.iter().enumerate() {
            let rings = gpu_rings(&levels.rings, index);
            draw_ring_chart(pen, gpu_grid_spot(index), gpu, rings);
        }
    }

    /// RAM use and SSD temperatures, on one line.
    fn draw_info(&self, pen: &mut Pen<'_, '_>) {
        if let Some(memory) = self.snapshot.memory.filter(|m| m.total_bytes > 0)
        {
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
        let text = disks.join("     ");
        let at = (COLUMN_W + COLUMN_GAP, HEAD_BASELINE);
        pen.text(at, HEAD_SIZE, QUIET, Align::Left, &text);
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

    /// A line through points.
    fn line(&mut self, points: &[(f64, f64)], paint: Paint) {
        let (points, weight) = (self.points(points), self.weight());
        self.canvas.polyline(&points, weight, paint);
    }

    /// The area under a line, down to `base`.
    fn area_under(&mut self, points: &[(f64, f64)], base: f64, paint: Paint) {
        let points = self.points(points);
        let (_, base) = self.point((0.0, base));
        self.canvas.fill_under(&points, base, paint);
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
        });
    }
}

#[cfg(test)]
mod tests {
    use super::{
        Dashboard, NET_SPAN, Ring, Widget, format_rate, ring_reading,
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

    #[test]
    fn keeps_a_minute_of_network_history() {
        let mut dashboard = Dashboard::default();
        let extra = 40_u32;
        let total = u32::try_from(NET_SPAN).unwrap() + extra;
        for i in 0..total {
            dashboard.update(Snapshot {
                network: Some(Network {
                    down_kbps: f64::from(i),
                    up_kbps: 0.0,
                }),
                ..Snapshot::default()
            });
        }
        assert_eq!(dashboard.net.len(), NET_SPAN);
        let oldest = Some(f64::from(extra));
        assert_eq!(dashboard.net.front().map(|n| n.down_kbps), oldest);
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
}
