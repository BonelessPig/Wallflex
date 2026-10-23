//! The widgets: CPU thread columns, the network graph and the GPU rings,
//! redrawn natively from the user's wallpaper designs (`cpu-cores.js`,
//! `net-graph.js`, `gpu-rings.js` and their CSS).
//!
//! Sizes are in design units: 1 unit = 0.1% of the monitor's height (the
//! wallpaper's 0.1vh), which is also the line weight. Shapes go to a
//! [`Canvas`]; text comes back as [`Label`]s for GDI to draw on top.

use std::collections::VecDeque;

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

/// Network: samples kept, one per second.
pub const NET_SPAN: usize = 60;
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

/// What the widgets show: the latest reading plus the network history.
#[derive(Default)]
pub struct Dashboard {
    /// The latest reading.
    snapshot: Snapshot,
    /// The last [`NET_SPAN`] network readings, oldest first.
    net: VecDeque<Network>,
}

impl Dashboard {
    /// Takes in a new reading.
    pub fn update(&mut self, snapshot: Snapshot) {
        if let Some(network) = snapshot.network {
            if self.net.len() >= NET_SPAN {
                let _oldest = self.net.pop_front();
            }
            self.net.push_back(network);
        }
        self.snapshot = snapshot;
    }

    /// The widgets' size in design units.
    pub fn size(&self) -> (f64, f64) {
        (
            COLUMN_W * 2.0 + COLUMN_GAP,
            self.info_baseline() + HEAD_SIZE / 2.0,
        )
    }

    /// Draws everything into `canvas` (already sized and cleared to
    /// `background`) at `scale` pixels per unit. Returns the text to draw
    /// on top.
    pub fn draw(
        &self,
        canvas: &mut Canvas<'_>,
        scale: f64,
        background: Rgb,
    ) -> Vec<Label> {
        let mut pen = Pen {
            canvas,
            scale,
            background,
            labels: Vec::new(),
        };
        self.draw_cpu(&mut pen);
        self.draw_net(&mut pen, COLUMN_W + COLUMN_GAP);
        self.draw_rings(&mut pen);
        self.draw_info(&mut pen);
        pen.labels
    }

    /// Top of the ring charts' area (below the first row and the names).
    fn rings_top() -> f64 {
        BODY_TOP + NET_GRAPH_H + ROW_GAP + RING_NAME_ROOM
    }

    /// Baseline of the RAM/SSD line under the rings.
    fn info_baseline(&self) -> f64 {
        let rows = self.snapshot.gpus.len().div_ceil(RINGS_PER_ROW);
        let rings_h =
            count_to_f64(rows) * (RING_SIZE + ROW_GAP + RING_NAME_ROOM);
        Self::rings_top() + rings_h + HEAD_BASELINE
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

        let core_n = count_to_f64(cores);
        let core_w = (COLUMN_W - CPU_CORE_GAP * (core_n - 1.0)) / core_n;
        let per_core_n = count_to_f64(per_core);
        let col_w = (core_w - CPU_PAIR_GAP * (per_core_n - 1.0)) / per_core_n;
        let col_h = CPU_COLS_H - CPU_LABEL_GAP - CPU_LABEL_SIZE * 1.2;

        for (core, loads) in (1_u32..).zip(cpu.per_core.chunks(per_core)) {
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

    /// The network graph, in the second column.
    fn draw_net(&self, pen: &mut Pen<'_, '_>, left: f64) {
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
        self.draw_net_graph(pen, left);
    }

    /// The network graph's plot: the last minute of download (white,
    /// faint fill) and upload (blue).
    fn draw_net_graph(&self, pen: &mut Pen<'_, '_>, left: f64) {
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
    fn draw_rings(&self, pen: &mut Pen<'_, '_>) {
        for (row, pair) in
            (0_u32..).zip(self.snapshot.gpus.chunks(RINGS_PER_ROW))
        {
            let top = Self::rings_top()
                + f64::from(row) * (RING_SIZE + ROW_GAP + RING_NAME_ROOM);
            for (column, gpu) in (0_u32..).zip(pair) {
                let left = f64::from(column) * (COLUMN_W + COLUMN_GAP);
                draw_ring_chart(pen, (left, top), gpu);
            }
        }
    }

    /// RAM use and SSD temperatures, on one line under the rings.
    fn draw_info(&self, pen: &mut Pen<'_, '_>) {
        let mut parts = Vec::new();
        if let Some(memory) = self.snapshot.memory.filter(|m| m.total_bytes > 0)
        {
            let used = u64_to_f64(memory.used_bytes);
            let total = u64_to_f64(memory.total_bytes);
            parts.push(format!(
                "RAM {:.0}%  {:.1}/{:.1} GB",
                used / total * PERCENT_MAX,
                used / BYTES_PER_GB,
                total / BYTES_PER_GB
            ));
        }
        for disk in &self.snapshot.disks {
            parts.push(format!("SSD{} {} °C", disk.disk, disk.temp_c));
        }
        let text = parts.join("     ");
        pen.text(
            (0.0, self.info_baseline()),
            HEAD_SIZE,
            QUIET,
            Align::Left,
            &text,
        );
    }
}

/// Draws one GPU's rings, names and legend with its top-left at
/// `origin`.
fn draw_ring_chart(pen: &mut Pen<'_, '_>, (left, top): (f64, f64), gpu: &Gpu) {
    let to_units = RING_SIZE / RING_VIEW;
    let centre = (left + RING_SIZE / 2.0, top + RING_SIZE / 2.0);
    for (index, ring) in (0_u32..).zip(Ring::ALL) {
        let radius = (RING_OUTER_R - f64::from(index) * RING_GAP) * to_units;
        let (fraction, _) = ring_reading(gpu, ring);
        pen.arc(
            centre,
            radius,
            fraction.unwrap_or(0.0),
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
    /// Text collected for GDI.
    labels: Vec<Label>,
}

impl Pen<'_, '_> {
    /// An area from units to pixels.
    fn px(&self, area: Area) -> Area {
        Area {
            x0: area.x0 * self.scale,
            y0: area.y0 * self.scale,
            x1: area.x1 * self.scale,
            y1: area.y1 * self.scale,
        }
    }

    /// Points from units to pixels.
    fn points(&self, points: &[(f64, f64)]) -> Vec<(f64, f64)> {
        points
            .iter()
            .map(|&(x, y)| (x * self.scale, y * self.scale))
            .collect()
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
        self.canvas.dashed_hline(
            (x0 * s, x1 * s),
            y * s,
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
        self.canvas.fill_under(&points, base * self.scale, paint);
    }

    /// A ring arc.
    fn arc(
        &mut self,
        (cx, cy): (f64, f64),
        radius: f64,
        fraction: f64,
        paint: Paint,
    ) {
        let s = self.scale;
        let weight = self.weight();
        self.canvas
            .arc((cx * s, cy * s), radius * s, weight, fraction, paint);
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
        self.labels.push(Label {
            x: x * self.scale,
            y: y * self.scale,
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
        Dashboard, NET_SPAN, Ring, format_rate, ring_reading, split_gpu_name,
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
        for i in 0..100_u32 {
            dashboard.update(Snapshot {
                network: Some(Network {
                    down_kbps: f64::from(i),
                    up_kbps: 0.0,
                }),
                ..Snapshot::default()
            });
        }
        assert_eq!(dashboard.net.len(), NET_SPAN);
        assert_eq!(dashboard.net.front().map(|n| n.down_kbps), Some(40.0));
    }

    #[test]
    fn grows_a_row_per_two_gpus() {
        let mut dashboard = Dashboard::default();
        let (_, empty) = dashboard.size();
        dashboard.update(Snapshot {
            gpus: vec![Gpu::default(), Gpu::default()],
            ..Snapshot::default()
        });
        let (_, two) = dashboard.size();
        dashboard.update(Snapshot {
            gpus: vec![Gpu::default(); 3],
            ..Snapshot::default()
        });
        let (_, three) = dashboard.size();
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
        let (w, h) = dashboard.size();
        assert!(w * scale < 700.0 && h * scale < 450.0);
        let mut buffer = vec![0_u32; 700 * 450];
        let labels = {
            let mut canvas = Canvas::new(700, 450, &mut buffer).unwrap();
            dashboard.draw(&mut canvas, scale, Rgb::BLACK)
        };
        let texts: Vec<&str> = labels.iter().map(|l| l.text.as_str()).collect();
        assert!(texts.contains(&"CPU THREADS"));
        assert!(texts.contains(&"4.30 GHZ · 6C / 12T"));
        assert!(texts.contains(&"C6"));
        assert!(texts.contains(&"2080"));
        assert!(texts.contains(&"RTX"));
        assert!(texts.contains(&"NOW"));
        assert!(buffer.iter().any(|&p| p != 0), "something drawn");
    }
}
