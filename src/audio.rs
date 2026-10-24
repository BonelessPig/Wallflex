//! Sound to visualizer bars: converts captured samples to mono, keeps
//! the latest ones, and splits them into [`BANDS`] frequency bands with
//! an FFT. No allocation after setup and no recursion.

use std::collections::VecDeque;
use std::f64::consts::TAU;

/// Bars in the visualizer (as the wallpaper's).
pub const BANDS: usize = 32;
/// Samples per analysis (a power of two): about 85 ms at 48 kHz, so
/// steps of about 12 Hz, fine enough for the lowest bass bands.
pub const FFT_SIZE: usize = 4096;
/// Lowest frequency shown, in Hz (the bottom of human hearing).
const LOW_HZ: f64 = 20.0;
/// Highest frequency shown, in Hz (the top of human hearing).
const HIGH_HZ: f64 = 20_000.0;
/// Quietest level that shows, in dB below full scale.
const FLOOR_DB: f64 = -70.0;
/// Music has less energy up high; this many dB per octave above 1 kHz
/// (and less below) evens the bars out.
const TILT_DB_PER_OCTAVE: f64 = 3.0;
/// The frequency the tilt pivots on, in Hz.
const TILT_PIVOT_HZ: f64 = 1_000.0;
/// A Hann window halves a sine's amplitude; this undoes it.
const HANN_GAIN: f64 = 0.5;
/// Below this a sample counts as silence.
pub const SILENCE: f32 = 1.0e-4;
/// Largest 16-bit sample, for scaling to -1..1.
const I16_SCALE: f32 = 32_768.0;
/// Largest 32-bit sample, for scaling to -1..1.
const I32_SCALE: f32 = 2_147_483_648.0;

/// How the sound card's samples are stored.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SampleKind {
    /// 32-bit float, -1 to 1 (what Windows' mixer normally uses).
    F32,
    /// 16-bit integer.
    I16,
    /// 32-bit integer (also 24-bit audio in 32-bit slots).
    I32,
}

impl SampleKind {
    /// Bytes per sample.
    pub fn bytes(self) -> usize {
        match self {
            Self::F32 | Self::I32 => 4,
            Self::I16 => 2,
        }
    }

    /// One sample from its little-endian bytes, scaled to -1..1. 0 if
    /// `bytes` is the wrong length.
    fn read(self, bytes: &[u8]) -> f32 {
        match self {
            Self::F32 => {
                <[u8; 4]>::try_from(bytes).map_or(0.0, f32::from_le_bytes)
            }
            Self::I16 => <[u8; 2]>::try_from(bytes)
                .map_or(0.0, |b| f32::from(i16::from_le_bytes(b)) / I16_SCALE),
            Self::I32 => <[u8; 4]>::try_from(bytes).map_or(0.0, |b| {
                // Precision beyond f32's isn't needed for bars.
                #[expect(
                    clippy::as_conversions,
                    clippy::cast_precision_loss,
                    reason = "audio samples only need f32 precision"
                )]
                let sample = i32::from_le_bytes(b) as f32;
                sample / I32_SCALE
            }),
        }
    }
}

/// A capture format: sample type and channel count.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Format {
    /// Sample type.
    pub kind: SampleKind,
    /// Channels per frame.
    pub channels: usize,
}

impl Format {
    /// Bytes per frame (one sample per channel).
    pub fn frame_bytes(self) -> usize {
        self.kind.bytes().saturating_mul(self.channels)
    }
}

/// The latest samples, mixed to mono, oldest first.
#[derive(Debug)]
pub struct Recent {
    /// Up to [`FFT_SIZE`] samples.
    samples: VecDeque<f32>,
}

impl Default for Recent {
    fn default() -> Self {
        Self {
            samples: VecDeque::with_capacity(FFT_SIZE),
        }
    }
}

impl Recent {
    /// Adds interleaved frames in `format`, mixed to mono. Returns the
    /// loudest sample, to tell sound from silence.
    pub fn push_frames(&mut self, bytes: &[u8], format: Format) -> f32 {
        let size = format.kind.bytes();
        let frame = format.frame_bytes();
        if size == 0 || frame == 0 {
            return 0.0;
        }
        let channels = u16::try_from(format.channels).map_or(1.0, f32::from);
        let mut peak = 0.0_f32;
        for frame in bytes.chunks_exact(frame) {
            let sum: f32 =
                frame.chunks_exact(size).map(|s| format.kind.read(s)).sum();
            let mono = sum / channels;
            peak = peak.max(mono.abs());
            self.push(mono);
        }
        peak
    }

    /// Adds `count` silent samples (Windows marks quiet packets).
    pub fn push_silence(&mut self, count: usize) {
        for _ in 0..count.min(FFT_SIZE) {
            self.push(0.0);
        }
    }

    /// Adds one sample, dropping the oldest when full.
    fn push(&mut self, sample: f32) {
        if self.samples.len() >= FFT_SIZE {
            let _oldest = self.samples.pop_front();
        }
        self.samples.push_back(sample);
    }

    /// Copies the samples into `out`, oldest first.
    pub fn copy_to(&self, out: &mut Vec<f32>) {
        out.clear();
        out.extend(self.samples.iter().copied());
    }
}

/// An FFT set up once for [`FFT_SIZE`] samples.
pub struct Spectrum {
    /// Hann window weights.
    window: Vec<f64>,
    /// `e^(-2πik/N)` for k below N/2, as (re, im).
    twiddles: Vec<(f64, f64)>,
    /// Working buffer, (re, im).
    buffer: Vec<(f64, f64)>,
}

impl Default for Spectrum {
    fn default() -> Self {
        let n = count_f64(FFT_SIZE);
        let window = (0..FFT_SIZE)
            .map(|i| 0.5 - 0.5 * (TAU * count_f64(i) / (n - 1.0)).cos())
            .collect();
        let twiddles = (0..FFT_SIZE / 2)
            .map(|k| {
                let angle = -TAU * count_f64(k) / n;
                (angle.cos(), angle.sin())
            })
            .collect();
        Self {
            window,
            twiddles,
            buffer: vec![(0.0, 0.0); FFT_SIZE],
        }
    }
}

impl Spectrum {
    /// Each band's level (0 to 1) for `samples` (the newest
    /// [`FFT_SIZE`], fewer are padded with silence) at `rate` Hz.
    pub fn bands(&mut self, samples: &[f32], rate: u32) -> [f64; BANDS] {
        let start = samples.len().saturating_sub(FFT_SIZE);
        let recent = samples.get(start..).unwrap_or_default();
        let pad = FFT_SIZE.saturating_sub(recent.len());
        let input = std::iter::repeat_n(0.0, pad)
            .chain(recent.iter().map(|s| f64::from(*s)));
        for ((slot, sample), weight) in
            self.buffer.iter_mut().zip(input).zip(&self.window)
        {
            *slot = (sample * weight, 0.0);
        }
        fft(&mut self.buffer, &self.twiddles);
        let bin_hz = f64::from(rate) / count_f64(FFT_SIZE);
        let scale = 2.0 / (count_f64(FFT_SIZE) * HANN_GAIN);
        let half = self.buffer.get(..FFT_SIZE / 2).unwrap_or_default();
        let mut levels = [0.0; BANDS];
        for (index, level) in levels.iter_mut().enumerate() {
            let (low, high) = band_edges(index);
            let first = bin_of(low, bin_hz).max(1);
            let last = bin_of(high, bin_hz).max(first.saturating_add(1));
            let peak = half
                .get(first..last.min(half.len()))
                .unwrap_or_default()
                .iter()
                .map(|(re, im)| re.hypot(*im) * scale)
                .fold(0.0, f64::max);
            let centre = (low * high).sqrt();
            let tilt = TILT_DB_PER_OCTAVE * (centre / TILT_PIVOT_HZ).log2();
            let db = 20.0 * peak.max(f64::MIN_POSITIVE).log10() + tilt;
            *level = ((db - FLOOR_DB) / -FLOOR_DB).clamp(0.0, 1.0);
        }
        levels
    }
}

/// Where `hz` sits along the bands, from 0 (the lowest shown) to 1 (the
/// highest), on the same log scale as the bands.
pub fn band_position(hz: f64) -> f64 {
    ((hz / LOW_HZ).ln() / (HIGH_HZ / LOW_HZ).ln()).clamp(0.0, 1.0)
}

/// Band `index`'s frequency range in Hz: log-spaced, like hearing.
fn band_edges(index: usize) -> (f64, f64) {
    let ratio = HIGH_HZ / LOW_HZ;
    let edge = |i: usize| LOW_HZ * ratio.powf(count_f64(i) / count_f64(BANDS));
    (edge(index), edge(index.saturating_add(1)))
}

/// The FFT bin holding `hz`.
#[expect(
    clippy::as_conversions,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "clamped to a small non-negative range first"
)]
fn bin_of(hz: f64, bin_hz: f64) -> usize {
    let bin = (hz / bin_hz).round();
    if bin.is_finite() {
        bin.clamp(0.0, count_f64(FFT_SIZE)) as usize
    } else {
        0
    }
}

/// In-place radix-2 FFT of `data` (length a power of two, twiddles for
/// that length). Iterative: bit-reversal, then butterflies stage by
/// stage.
fn fft(data: &mut [(f64, f64)], twiddles: &[(f64, f64)]) {
    let n = data.len();
    if n < 2 || !n.is_power_of_two() {
        return;
    }
    let bits = n.trailing_zeros();
    for i in 0..n {
        let j = i.reverse_bits() >> (usize::BITS.saturating_sub(bits));
        if i < j {
            data.swap(i, j);
        }
    }
    let mut len = 2;
    while len <= n {
        let half = len / 2;
        let stride = n.checked_div(len).unwrap_or(1);
        for chunk in data.chunks_exact_mut(len) {
            let (evens, odds) = chunk.split_at_mut(half);
            let steps = twiddles.iter().step_by(stride);
            for ((even, odd), &(wr, wi)) in
                evens.iter_mut().zip(odds).zip(steps)
            {
                let (or, oi) = *odd;
                let t = (or * wr - oi * wi, or * wi + oi * wr);
                let e = *even;
                *even = (e.0 + t.0, e.1 + t.1);
                *odd = (e.0 - t.0, e.1 - t.1);
            }
        }
        len = len.saturating_mul(2);
    }
}

/// A small count as `f64`.
fn count_f64(count: usize) -> f64 {
    u32::try_from(count).map_or(f64::MAX, f64::from)
}

#[cfg(test)]
mod tests {
    use super::{
        BANDS, FFT_SIZE, Format, Recent, SampleKind, Spectrum, band_edges,
        band_position,
    };

    #[test]
    fn positions_follow_the_bands() {
        assert!(band_position(20.0).abs() < 1e-9);
        assert!((band_position(20_000.0) - 1.0).abs() < 1e-9);
        let (low, _) = band_edges(BANDS / 2);
        assert!((band_position(low) - 0.5).abs() < 1e-9);
        assert!(band_position(10.0).abs() < 1e-9);
    }
    use std::f64::consts::TAU;

    #[test]
    fn mixes_stereo_to_mono() {
        let mut recent = Recent::default();
        let format = Format {
            kind: SampleKind::F32,
            channels: 2,
        };
        let mut bytes = Vec::new();
        for sample in [0.5_f32, -0.25, 1.0, 0.0] {
            bytes.extend_from_slice(&sample.to_le_bytes());
        }
        let peak = recent.push_frames(&bytes, format);
        let mut out = Vec::new();
        recent.copy_to(&mut out);
        assert_eq!(out, vec![0.125, 0.5]);
        assert!((peak - 0.5).abs() < 1e-6);

        let pcm = Format {
            kind: SampleKind::I16,
            channels: 1,
        };
        let _ = recent.push_frames(&i16::MIN.to_le_bytes(), pcm);
        recent.copy_to(&mut out);
        assert_eq!(out.last(), Some(&-1.0));
    }

    #[test]
    fn keeps_only_the_latest() {
        let mut recent = Recent::default();
        recent.push_silence(FFT_SIZE + 10);
        let format = Format {
            kind: SampleKind::F32,
            channels: 1,
        };
        let _ = recent.push_frames(&1.0_f32.to_le_bytes(), format);
        let mut out = Vec::new();
        recent.copy_to(&mut out);
        assert_eq!(out.len(), FFT_SIZE);
        assert_eq!(out.last(), Some(&1.0));
    }

    #[test]
    fn a_tone_lights_its_band() {
        let rate = 48_000;
        let band = 16;
        let (low, high) = band_edges(band);
        let hz = (low * high).sqrt();
        let samples: Vec<f32> = (0..FFT_SIZE)
            .map(|i| {
                let t = f64::from(u32::try_from(i).unwrap()) / f64::from(rate);
                #[expect(
                    clippy::as_conversions,
                    clippy::cast_possible_truncation
                )]
                let s = (0.5 * (TAU * hz * t).sin()) as f32;
                s
            })
            .collect();
        let levels = Spectrum::default().bands(&samples, rate);
        let loudest = (0..BANDS)
            .max_by(|a, b| levels[*a].total_cmp(&levels[*b]))
            .unwrap();
        assert_eq!(loudest, band);
        assert!(levels[band] > 0.8);
        assert!(levels[2] < 0.3, "low bands stay quiet: {levels:?}");
    }

    #[test]
    fn silence_is_flat() {
        let levels = Spectrum::default().bands(&[0.0; 100], 48_000);
        assert!(levels.iter().all(|l| *l == 0.0));
    }
}
