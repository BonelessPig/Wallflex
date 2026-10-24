//! The music visualizer: [`BANDS`] bars, sized like the wallpaper's
//! (0.7vh wide, 1.104vh apart, up to 7vh tall), with frequency labels
//! underneath. Bars jump up with the music and fall back smoothly; each
//! is shaded by loudness, green at the bottom through yellow to red at
//! the top, and a dim stub marks its place.

use super::{Align, CPU_LABEL_GAP, CPU_LABEL_SIZE, INK, Pen, QUIET, TRACK};
use crate::audio::{BANDS, Spectrum, band_position};
use crate::canvas::{Area, Paint, mix};
use crate::color::Rgb;

/// Bar width (0.7vh).
const BAR_W: f64 = 7.0;
/// Distance from one bar to the next (1.104vh).
const BAR_PITCH: f64 = 11.04;
/// Tallest bar (7vh).
pub const HEIGHT: f64 = 70.0;
/// The stub under each bar.
const STUB_H: f64 = 1.0;
/// Share of the way to a louder level a bar moves each frame.
const RISE: f64 = 0.6;
/// How far a bar falls per frame, as a share of the full height.
const FALL: f64 = 0.05;
/// Below this every bar counts as resting.
const RESTING: f64 = 0.002;
/// Labelled frequencies (Hz) and their labels.
const LABELS: [(f64, &str); 10] = [
    (20.0, "20"),
    (50.0, "50"),
    (100.0, "100"),
    (200.0, "200"),
    (500.0, "500"),
    (1_000.0, "1K"),
    (2_000.0, "2K"),
    (5_000.0, "5K"),
    (10_000.0, "10K"),
    (20_000.0, "20K"),
];
/// The loudness colours from quiet (bottom) to loud (top), with where
/// each sits (0 = bottom, 1 = top); colours between are blended.
const SHADES: [(f64, Rgb); 4] = [
    (
        0.0,
        Rgb {
            r: 0x6E,
            g: 0xE0,
            b: 0x8F,
        },
    ),
    (
        0.5,
        Rgb {
            r: 0xE8,
            g: 0xE4,
            b: 0x6A,
        },
    ),
    (
        0.75,
        Rgb {
            r: 0xFF,
            g: 0xB0,
            b: 0x5C,
        },
    ),
    (
        1.0,
        Rgb {
            r: 0xFF,
            g: 0x5F,
            b: 0x5F,
        },
    ),
];

/// The widget's height: bars plus the labels under them.
pub fn total_height() -> f64 {
    HEIGHT + CPU_LABEL_GAP + CPU_LABEL_SIZE * 1.2
}

/// The colour for a point `height` of the way up (0 to 1).
fn shade(height: f64) -> Rgb {
    let h = height.clamp(0.0, 1.0);
    let mut below = SHADES[0];
    for stop in SHADES {
        if h <= stop.0 {
            let span = stop.0 - below.0;
            let amount = if span > 0.0 {
                (h - below.0) / span
            } else {
                1.0
            };
            return mix(stop.1, below.1, amount);
        }
        below = stop;
    }
    below.1
}

/// The visualizer's width.
pub fn width() -> f64 {
    let gaps = u32::try_from(BANDS.saturating_sub(1)).map_or(0.0, f64::from);
    gaps * BAR_PITCH + BAR_W
}

/// The bars' state.
pub struct Visualizer {
    /// Each bar's height, 0 to 1.
    levels: [f64; BANDS],
    /// The analysis, set up once.
    spectrum: Spectrum,
}

impl Default for Visualizer {
    fn default() -> Self {
        Self {
            levels: [0.0; BANDS],
            spectrum: Spectrum::default(),
        }
    }
}

impl Visualizer {
    /// One frame: moves the bars toward `samples`' levels (mono, newest
    /// last, at `rate` Hz).
    pub fn feed(&mut self, samples: &[f32], rate: u32) {
        let target = self.spectrum.bands(samples, rate);
        for (level, target) in self.levels.iter_mut().zip(target) {
            *level = if target > *level {
                *level + (target - *level) * RISE
            } else {
                (*level - FALL).max(target)
            };
        }
    }

    /// Whether every bar has come to rest (no need for more frames).
    pub fn is_resting(&self) -> bool {
        self.levels.iter().all(|l| *l < RESTING)
    }

    /// Drops every bar (when the music is paused for a game, say).
    pub fn rest(&mut self) {
        self.levels = [0.0; BANDS];
    }

    /// Sample bars for `--preview`: a gentle hill.
    pub fn show_demo(&mut self) {
        for (index, level) in (0_u32..).zip(self.levels.iter_mut()) {
            let x = f64::from(index) / 31.0;
            *level = 0.25
                + 0.75 * (x * std::f64::consts::PI).sin() * (1.0 - x * 0.4);
        }
    }

    /// Draws the bars from the bottom up.
    pub fn draw(&self, pen: &mut Pen<'_, '_>) {
        for (index, level) in (0_u32..).zip(self.levels) {
            let x0 = f64::from(index) * BAR_PITCH;
            let x1 = x0 + BAR_W;
            pen.fill(
                Area {
                    x0,
                    y0: HEIGHT - STUB_H,
                    x1,
                    y1: HEIGHT,
                },
                Paint {
                    color: INK,
                    alpha: TRACK,
                },
            );
            let top = HEIGHT - level.clamp(0.0, 1.0) * HEIGHT;
            if top < HEIGHT {
                let bar = Area {
                    x0,
                    y0: top,
                    x1,
                    y1: HEIGHT,
                };
                pen.fill_shaded(bar, |y| shade((HEIGHT - y) / HEIGHT));
            }
        }
        draw_labels(pen);
    }
}

/// The frequency labels under the bars, each under its frequency; the
/// end ones line up with the edges.
fn draw_labels(pen: &mut Pen<'_, '_>) {
    let width = width();
    let baseline = HEIGHT + CPU_LABEL_GAP + CPU_LABEL_SIZE;
    let last = LABELS.len().saturating_sub(1);
    for (index, (hz, label)) in LABELS.iter().enumerate() {
        let x = band_position(*hz) * width;
        let align = match index {
            0 => Align::Left,
            i if i == last => Align::Right,
            _ => Align::Center,
        };
        pen.text((x, baseline), CPU_LABEL_SIZE, QUIET, align, label);
    }
}

#[cfg(test)]
mod tests {
    use super::{BANDS, SHADES, Visualizer, shade};

    #[test]
    fn shades_run_green_to_red() {
        assert_eq!(shade(0.0), SHADES[0].1);
        assert_eq!(shade(1.0), SHADES[3].1);
        assert_eq!(shade(0.5), SHADES[1].1);
        let between = shade(0.25);
        assert!(between.r > SHADES[0].1.r && between.r < SHADES[1].1.r);
    }

    #[test]
    fn rises_fast_and_falls_slowly() {
        let mut bars = Visualizer {
            levels: [1.0; BANDS],
            ..Visualizer::default()
        };
        bars.feed(&[0.0; 64], 48_000);
        assert!(bars.levels.iter().all(|l| (l - 0.95).abs() < 1e-9));
        assert!(!bars.is_resting());
        for _ in 0..30 {
            bars.feed(&[0.0; 64], 48_000);
        }
        assert!(bars.is_resting());
    }
}
