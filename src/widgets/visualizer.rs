//! The music visualizer: [`BANDS`] bars, sized like the wallpaper's
//! (0.7vh wide, 1.104vh apart, up to 7vh tall), with frequency labels
//! underneath. Bars jump up with the music and fall back smoothly; each
//! is shaded by loudness, green at the bottom through yellow to red at
//! the top, and a dim stub marks its place. When something's playing, its
//! album art or thumbnail sits to the left and its title (scrolling when
//! it's too long) and artist underneath.

use std::time::Duration;

use super::{Align, CPU_LABEL_GAP, CPU_LABEL_SIZE, INK, Pen, QUIET, TRACK};
use crate::audio::{BANDS, Spectrum, band_position};
use crate::canvas::{Area, Paint, mix};
use crate::color::Rgb;
use crate::media::{MediaChange, NowPlaying};

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
/// The thumbnail's gap to the bars.
const THUMB_GAP: f64 = 12.0;
/// The thumbnail's corner radius.
const THUMB_CORNER: f64 = 6.0;
/// Gap between the frequency labels and the title.
const NOW_GAP: f64 = 8.0;
/// Title text size.
const TITLE_SIZE: f64 = 10.5;
/// Artist text size.
const ARTIST_SIZE: f64 = 9.0;
/// From the title's baseline to the artist's.
const ARTIST_LINE: f64 = 12.0;
/// The two now-playing lines' height.
const NOW_H: f64 = NOW_GAP + TITLE_SIZE + ARTIST_LINE + 3.0;
/// How far letters reach below the baseline, as a share of the size.
const DESCENT: f64 = 0.22;
/// Approximate width of a character as a share of the text size.
const CHAR_WIDTH: f64 = 0.55;
/// How fast a long title scrolls, in units a second.
const SCROLL_SPEED: f64 = 30.0;
/// How long a long title rests at its start before scrolling.
const SCROLL_PAUSE: Duration = Duration::from_millis(1_500);
/// Gap between the end of a scrolling title and its next start.
const SCROLL_GAP: f64 = 40.0;
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

/// The colour for a point `height` of the way up (0 to 1): green through
/// yellow and orange to red. Also used for the network graph.
pub(super) fn shade(height: f64) -> Rgb {
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

/// The bars' width.
fn width() -> f64 {
    let gaps = u32::try_from(BANDS.saturating_sub(1)).map_or(0.0, f64::from);
    gaps * BAR_PITCH + BAR_W
}

/// The bars' state.
pub struct Visualizer {
    /// Each bar's height, 0 to 1.
    levels: [f64; BANDS],
    /// The analysis, set up once.
    spectrum: Spectrum,
    /// What's playing, if anything.
    now: Option<NowPlaying>,
    /// How far a long title has scrolled (units).
    scrolled: f64,
    /// How long it has rested at its start.
    rested: Duration,
}

impl Default for Visualizer {
    fn default() -> Self {
        Self {
            levels: [0.0; BANDS],
            spectrum: Spectrum::default(),
            now: None,
            scrolled: 0.0,
            rested: Duration::ZERO,
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

    /// What's playing changed.
    pub fn set_now_playing(&mut self, change: MediaChange) {
        match change {
            MediaChange::Nothing => self.now = None,
            MediaChange::Song(song) => {
                self.now = Some(song);
                self.scrolled = 0.0;
                self.rested = Duration::ZERO;
            }
            MediaChange::Playing(playing) => {
                if let Some(now) = self.now.as_mut() {
                    now.playing = playing;
                }
            }
        }
    }

    /// Moves a long title along by `elapsed`, resting at its start each
    /// time round.
    pub fn scroll(&mut self, elapsed: Duration) {
        let Some(loop_length) = self.scroll_length() else {
            return;
        };
        if self.rested < SCROLL_PAUSE {
            self.rested = self.rested.saturating_add(elapsed);
            return;
        }
        self.scrolled += SCROLL_SPEED * elapsed.as_secs_f64();
        if self.scrolled >= loop_length {
            self.scrolled = 0.0;
            self.rested = Duration::ZERO;
        }
    }

    /// One trip round a scrolling title (its width plus the gap), or
    /// `None` if it fits.
    fn scroll_length(&self) -> Option<f64> {
        let now = self.now.as_ref()?;
        let title = text_width(&now.title, TITLE_SIZE);
        (title > width()).then_some(title + SCROLL_GAP)
    }

    /// Where the bars start: right of the thumbnail when there is one.
    fn bars_left(&self) -> f64 {
        let thumbnail =
            self.now.as_ref().is_some_and(|n| n.thumbnail.is_some());
        if thumbnail {
            thumb_side() + THUMB_GAP
        } else {
            0.0
        }
    }

    /// The widget's size, now-playing lines and thumbnail included.
    pub fn size(&self) -> (f64, f64) {
        let lines = if self.now.is_some() { NOW_H } else { 0.0 };
        (self.bars_left() + width(), total_height() + lines)
    }

    /// The areas frames change: the bars, and a scrolling title.
    pub fn moving_areas(&self) -> Vec<Area> {
        let left = self.bars_left();
        let mut areas = vec![Area {
            x0: left,
            y0: 0.0,
            x1: left + width(),
            y1: HEIGHT,
        }];
        if self.scroll_length().is_some() {
            areas.push(self.title_box());
        }
        areas
    }

    /// The box the title shows in.
    fn title_box(&self) -> Area {
        let left = self.bars_left();
        let top = total_height() + NOW_GAP;
        Area {
            x0: left,
            y0: top,
            x1: left + width(),
            y1: top + TITLE_SIZE * 1.3,
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

    /// Sample bars for `--preview`: a gentle hill, with a sample song
    /// (a long title and a gradient for the thumbnail).
    pub fn show_demo(&mut self) {
        let edge = 64_usize;
        let bytes: Vec<u8> = (0..edge * edge)
            .flat_map(|i| {
                let column = i.checked_rem(edge).unwrap_or(0);
                let x =
                    u8::try_from(column.saturating_mul(4)).unwrap_or(u8::MAX);
                let row = i.checked_div(edge).unwrap_or(0);
                let y = u8::try_from(row.saturating_mul(4)).unwrap_or(u8::MAX);
                [y, 0x60, x, 0xFF]
            })
            .collect();
        self.now = Some(NowPlaying {
            title: "A song with a title long enough that it scrolls across"
                .into(),
            artist: "Some Artist".into(),
            playing: true,
            thumbnail: crate::image::Image::from_pbgra(edge, edge, &bytes),
        });
        for (index, level) in (0_u32..).zip(self.levels.iter_mut()) {
            let x = f64::from(index) / 31.0;
            *level = 0.25
                + 0.75 * (x * std::f64::consts::PI).sin() * (1.0 - x * 0.4);
        }
    }

    /// Draws the thumbnail, the bars from the bottom up, the frequency
    /// labels and what's playing.
    pub fn draw(&self, pen: &mut Pen<'_, '_>) {
        if let Some(image) =
            self.now.as_ref().and_then(|n| n.thumbnail.as_ref())
        {
            let half = thumb_side() / 2.0;
            pen.image_rounded((half, half), half, THUMB_CORNER, image);
        }
        let left = self.bars_left();
        let origin = pen.origin;
        pen.origin.0 += left;
        self.draw_bars(pen);
        draw_labels(pen);
        pen.origin = origin;
        self.draw_now_playing(pen);
    }

    /// The title (scrolling if it's too long) and artist under the
    /// labels; dimmed while paused.
    fn draw_now_playing(&self, pen: &mut Pen<'_, '_>) {
        let Some(now) = &self.now else {
            return;
        };
        let alpha = if now.playing { 1.0 } else { QUIET };
        let clip = self.title_box();
        let baseline = clip.y0 + TITLE_SIZE;
        let x = clip.x0 - self.scrolled;
        pen.text_clipped((x, baseline), TITLE_SIZE, alpha, clip, &now.title);
        if let Some(loop_length) = self.scroll_length() {
            // The next time round, following on.
            let next = (x + loop_length, baseline);
            pen.text_clipped(next, TITLE_SIZE, alpha, clip, &now.title);
        }
        let artist_box = Area {
            y0: clip.y1,
            y1: clip.y1 + ARTIST_LINE,
            ..clip
        };
        let artist_baseline = baseline + ARTIST_LINE;
        pen.text_clipped(
            (clip.x0, artist_baseline),
            ARTIST_SIZE,
            QUIET,
            artist_box,
            &now.artist,
        );
    }

    /// The bars and their stubs.
    fn draw_bars(&self, pen: &mut Pen<'_, '_>) {
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
    }
}

/// The thumbnail's side: from the top of the bars to the bottom of the
/// artist's name (so it lines up with both).
fn thumb_side() -> f64 {
    let artist_baseline = total_height() + NOW_GAP + TITLE_SIZE + ARTIST_LINE;
    artist_baseline + ARTIST_SIZE * DESCENT
}

/// Roughly how wide `text` is at `size`.
fn text_width(text: &str, size: f64) -> f64 {
    let chars = u32::try_from(text.chars().count()).map_or(f64::MAX, f64::from);
    chars * size * CHAR_WIDTH
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
