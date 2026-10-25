//! How smoothly Wallflex animates. The config's `performance` mode sets
//! the most it may do; the [`Governor`] picks the [`Level`] it runs at
//! from moment to moment: the mode's top level until the PC's CPU or
//! RAM is 90% used, then Light, and back up once things have been calm
//! for a while.

use std::time::Duration;

use crate::widgets::{Ease, Pace};

/// The config's `performance` setting.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Mode {
    /// Smooth animations when the PC has room, less when it's busy.
    #[default]
    Smooth,
    /// No glides, everything read once a second, a slow visualizer: as
    /// light as Wallflex gets.
    Light,
}

/// How much animating Wallflex does right now, lightest first.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    /// No glides; sensors once a second; visualizer ~16 fps.
    Light,
    /// 0.3 s glides at ~30 fps; CPU and network twice a second;
    /// visualizer ~30 fps.
    Normal,
    /// Continuous glides at ~60 fps (each lasts until the next reading,
    /// at an even speed); CPU and network four times a second;
    /// visualizer ~60 fps.
    Smooth,
}

// Windows timers tick every 15.625 ms unless an app asks for finer, and
// a timer fires on the first tick after it's due. So intervals just
// under a whole number of ticks give the rate intended: 15 ms -> 64 fps,
// 31 ms -> 32 fps, 62 ms -> 16 fps.

/// Frame interval for ~60 fps, in milliseconds.
const MS_60_FPS: u32 = 15;
/// Frame interval for ~30 fps, in milliseconds.
const MS_30_FPS: u32 = 31;
/// Frame interval for ~16 fps, in milliseconds.
const MS_16_FPS: u32 = 62;
/// Sensor tick for four times a second, in milliseconds.
const MS_QUARTER_SECOND: u32 = 250;
/// Sensor tick for twice a second, in milliseconds.
const MS_HALF_SECOND: u32 = 500;
/// Sensor tick for once a second, in milliseconds.
const MS_SECOND: u32 = 1_000;

impl Level {
    /// The most `mode` allows.
    fn top(mode: Mode) -> Self {
        match mode {
            Mode::Smooth => Self::Smooth,
            Mode::Light => Self::Light,
        }
    }

    /// One step smoother (Smooth stays Smooth).
    fn higher(self) -> Self {
        match self {
            Self::Light => Self::Normal,
            Self::Normal | Self::Smooth => Self::Smooth,
        }
    }

    /// How long, and how, the CPU columns glide to a new reading; `None`
    /// means they jump. Smooth glides last until the next reading, so
    /// the columns keep moving instead of stepping.
    pub fn glide(self) -> Option<(Duration, Ease)> {
        match self {
            Self::Smooth => Some((
                Duration::from_millis(u64::from(MS_QUARTER_SECOND)),
                Ease::Linear,
            )),
            Self::Normal => Some((Duration::from_millis(300), Ease::Out)),
            Self::Light => None,
        }
    }

    /// How long the network graph takes to slide one reading along;
    /// `None` means it steps.
    pub fn net_scroll(self) -> Option<Duration> {
        match self {
            Self::Smooth => {
                Some(Duration::from_millis(u64::from(MS_QUARTER_SECOND)))
            }
            Self::Normal | Self::Light => None,
        }
    }

    /// The same for the GPU rings and RAM bar, which get a new reading
    /// once a second: smooth glides last that whole second.
    pub fn slow_glide(self) -> Option<(Duration, Ease)> {
        match self {
            Self::Smooth => Some((
                Duration::from_millis(u64::from(MS_SECOND)),
                Ease::Linear,
            )),
            Self::Normal | Self::Light => self.glide(),
        }
    }

    /// Time between glide frames, in milliseconds.
    pub fn frame_ms(self) -> u32 {
        match self {
            Self::Smooth => MS_60_FPS,
            Self::Normal | Self::Light => MS_30_FPS,
        }
    }

    /// Time between visualizer frames, in milliseconds.
    pub fn visualizer_ms(self) -> u32 {
        match self {
            Self::Smooth => MS_60_FPS,
            Self::Normal => MS_30_FPS,
            Self::Light => MS_16_FPS,
        }
    }

    /// Time between sensor ticks, in milliseconds. Every tick reads the
    /// CPU and network; a full reading happens once a second.
    pub fn sensor_ms(self) -> u32 {
        match self {
            Self::Smooth => MS_QUARTER_SECOND,
            Self::Normal => MS_HALF_SECOND,
            Self::Light => MS_SECOND,
        }
    }

    /// How the bars, rings and graph move at this level. In Smooth the
    /// CPU columns move every frame (~64 fps), the graph every second
    /// frame and the slow rings every fourth: they move less than a
    /// pixel a frame, so more frames would be wasted work.
    pub fn pace(self) -> Pace {
        let (net_every, slow_every) = match self {
            Self::Smooth => (2, 4),
            Self::Normal | Self::Light => (1, 1),
        };
        Pace {
            glide: self.glide(),
            slow_glide: self.slow_glide(),
            net_scroll: self.net_scroll(),
            readings_per_second: MS_SECOND
                .checked_div(self.sensor_ms())
                .and_then(|n| usize::try_from(n).ok())
                .unwrap_or(1),
            net_every,
            slow_every,
        }
    }

    /// For the log.
    pub fn name(self) -> &'static str {
        match self {
            Self::Smooth => "smooth",
            Self::Normal => "normal",
            Self::Light => "light",
        }
    }
}

/// Whole-PC CPU or RAM use (%) at which animations drop straight to
/// Light. Below it nothing is throttled.
const BUSY: f64 = 90.0;
/// Calm seconds (CPU and RAM both under [`BUSY`]) before stepping back
/// up a level.
const CALM_SECONDS: u32 = 10;

/// Picks the level from moment to moment. Call [`Governor::tick`] once
/// a second.
#[derive(Debug)]
pub struct Governor {
    /// The configured mode.
    mode: Mode,
    /// The level now.
    level: Level,
    /// Seconds in a row the PC has had room.
    calm: u32,
}

impl Governor {
    /// A governor for `mode`, starting at its top level.
    pub fn new(mode: Mode) -> Self {
        Self {
            mode,
            level: Level::top(mode),
            calm: 0,
        }
    }

    /// The level now.
    pub fn level(&self) -> Level {
        self.level
    }

    /// Switches mode (the config changed). Returns the new level if it
    /// changed.
    pub fn set_mode(&mut self, mode: Mode) -> Option<Level> {
        if mode == self.mode {
            return None;
        }
        *self = Self::new(mode);
        Some(self.level)
    }

    /// One second's check: `cpu` and `ram` are the whole PC's CPU and
    /// RAM use (%, if known). Returns the new level if it changed.
    pub fn tick(
        &mut self,
        cpu: Option<f64>,
        ram: Option<f64>,
    ) -> Option<Level> {
        let busy = [cpu, ram].into_iter().flatten().any(|use_| use_ >= BUSY);
        let top = Level::top(self.mode);
        let before = self.level;
        if busy {
            self.level = Level::Light;
            self.calm = 0;
        } else if self.level < top {
            // Step up only after a calm spell, one level at a time.
            self.calm = self.calm.saturating_add(1);
            if self.calm >= CALM_SECONDS {
                self.calm = 0;
                self.level = self.level.higher().min(top);
            }
        } else {
            self.calm = 0;
        }
        (self.level != before).then_some(self.level)
    }
}

#[cfg(test)]
mod tests {
    use super::{CALM_SECONDS, Governor, Level, Mode};

    #[test]
    fn light_mode_stays_light() {
        let mut governor = Governor::new(Mode::Light);
        assert_eq!(governor.level(), Level::Light);
        for _ in 0..100 {
            assert_eq!(governor.tick(Some(1.0), Some(40.0)), None);
        }
        assert!(governor.level().glide().is_none());
    }

    #[test]
    fn a_busy_cpu_steps_down_and_calm_steps_up() {
        let mut governor = Governor::new(Mode::Smooth);
        // Busy but under the limit: stays smooth.
        for _ in 0..100 {
            assert_eq!(governor.tick(Some(89.0), Some(50.0)), None);
        }
        // At it: straight to Light.
        assert_eq!(governor.tick(Some(90.0), Some(50.0)), Some(Level::Light));
        // Calm again: one level per calm spell.
        let mut changes = Vec::new();
        for _ in 0..CALM_SECONDS * 2 {
            changes.extend(governor.tick(Some(5.0), Some(50.0)));
        }
        assert_eq!(changes, vec![Level::Normal, Level::Smooth]);
    }

    #[test]
    fn full_ram_steps_down() {
        let mut governor = Governor::new(Mode::Smooth);
        assert_eq!(governor.tick(Some(5.0), Some(89.9)), None);
        assert_eq!(governor.tick(Some(5.0), Some(95.0)), Some(Level::Light));
        // Still full: stays down however long it lasts.
        for _ in 0..CALM_SECONDS * 3 {
            assert_eq!(governor.tick(Some(5.0), Some(95.0)), None);
        }
    }

    #[test]
    fn unknown_readings_dont_throttle() {
        let mut governor = Governor::new(Mode::Smooth);
        for _ in 0..100 {
            assert_eq!(governor.tick(None, None), None);
        }
        assert_eq!(governor.level(), Level::Smooth);
    }

    #[test]
    fn changing_mode_resets() {
        let mut governor = Governor::new(Mode::Smooth);
        assert_eq!(governor.set_mode(Mode::Light), Some(Level::Light));
        assert_eq!(governor.set_mode(Mode::Light), None);
    }
}
