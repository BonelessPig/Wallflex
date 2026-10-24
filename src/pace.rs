//! How smoothly Wallflex animates. The config's `performance` mode sets
//! the most it may do; the [`Governor`] picks the [`Level`] it runs at
//! from moment to moment: smooth while the PC is idle, stepping down
//! when the PC gets busy or Wallflex itself goes over its CPU budget,
//! and back up once things have been calm for a while.

use std::time::Duration;

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
    /// 0.4 s glides at ~60 fps; CPU and network twice a second;
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

    /// One step lighter (Light stays Light).
    fn lower(self) -> Self {
        match self {
            Self::Smooth => Self::Normal,
            Self::Normal | Self::Light => Self::Light,
        }
    }

    /// One step smoother (Smooth stays Smooth).
    fn higher(self) -> Self {
        match self {
            Self::Light => Self::Normal,
            Self::Normal | Self::Smooth => Self::Smooth,
        }
    }

    /// How long bars and rings glide to a new reading; `None` means they
    /// jump.
    pub fn glide(self) -> Option<Duration> {
        match self {
            Self::Smooth => Some(Duration::from_millis(400)),
            Self::Normal => Some(Duration::from_millis(300)),
            Self::Light => None,
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
            Self::Smooth | Self::Normal => MS_HALF_SECOND,
            Self::Light => MS_SECOND,
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

/// Whole-PC CPU use (%) at which animations step down to Normal.
const BUSY: f64 = 60.0;
/// Whole-PC CPU use (%) at which they step down to Light.
const VERY_BUSY: f64 = 85.0;
/// Wallflex's CPU budget, as a share of the whole CPU (%). Over it for
/// [`OVER_BUDGET_SECONDS`] in a row, it steps down a level.
pub const CPU_BUDGET: f64 = 2.0;
/// Seconds over budget before stepping down.
const OVER_BUDGET_SECONDS: u32 = 3;
/// Wallflex must be under this (% of the whole CPU) to step back up, so
/// it doesn't bounce off the budget.
const HEADROOM: f64 = 1.0;
/// Calm seconds before stepping back up a level.
const CALM_SECONDS: u32 = 10;

/// Picks the level from moment to moment. Call [`Governor::tick`] once
/// a second.
#[derive(Debug)]
pub struct Governor {
    /// The configured mode.
    mode: Mode,
    /// The level now.
    level: Level,
    /// The most the CPU budget allows, lowered when over budget.
    budget_cap: Level,
    /// Seconds in a row over budget.
    over: u32,
    /// Seconds in a row a higher level would have been allowed.
    calm: u32,
}

impl Governor {
    /// A governor for `mode`, starting at its top level.
    pub fn new(mode: Mode) -> Self {
        let top = Level::top(mode);
        Self {
            mode,
            level: top,
            budget_cap: Level::Smooth,
            over: 0,
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

    /// One second's check: `system` is the whole PC's CPU use (%, if
    /// known) and `own` Wallflex's share of the whole CPU (%). Returns
    /// the new level if it changed.
    pub fn tick(&mut self, system: Option<f64>, own: f64) -> Option<Level> {
        self.over = if own > CPU_BUDGET {
            self.over.saturating_add(1)
        } else {
            0
        };
        if self.over >= OVER_BUDGET_SECONDS {
            self.budget_cap = self.level.lower();
            self.over = 0;
        }
        let load_cap = match system {
            Some(busy) if busy >= VERY_BUSY => Level::Light,
            Some(busy) if busy >= BUSY => Level::Normal,
            _ => Level::Smooth,
        };
        let wanted = Level::top(self.mode).min(load_cap).min(self.budget_cap);
        let before = self.level;
        if wanted < self.level {
            // Step down at once.
            self.level = wanted;
            self.calm = 0;
        } else if wanted > self.level || self.budget_cap < Level::top(self.mode)
        {
            // Step up only after a calm spell, one level at a time, and
            // only with room under the budget.
            self.calm = if own < HEADROOM {
                self.calm.saturating_add(1)
            } else {
                0
            };
            if self.calm >= CALM_SECONDS {
                self.calm = 0;
                self.budget_cap = self.budget_cap.higher();
                let allowed =
                    Level::top(self.mode).min(load_cap).min(self.budget_cap);
                self.level = self.level.higher().min(allowed);
            }
        } else {
            self.calm = 0;
        }
        (self.level != before).then_some(self.level)
    }
}

#[cfg(test)]
mod tests {
    use super::{CALM_SECONDS, Governor, Level, Mode, OVER_BUDGET_SECONDS};

    #[test]
    fn light_mode_stays_light() {
        let mut governor = Governor::new(Mode::Light);
        assert_eq!(governor.level(), Level::Light);
        for _ in 0..100 {
            assert_eq!(governor.tick(Some(1.0), 0.1), None);
        }
        assert!(governor.level().glide().is_none());
    }

    #[test]
    fn a_busy_pc_steps_down_and_calm_steps_up() {
        let mut governor = Governor::new(Mode::Smooth);
        assert_eq!(governor.tick(Some(70.0), 0.5), Some(Level::Normal));
        assert_eq!(governor.tick(Some(95.0), 0.5), Some(Level::Light));
        // Calm again: one level per calm spell.
        let mut changes = Vec::new();
        for _ in 0..CALM_SECONDS * 2 {
            changes.extend(governor.tick(Some(5.0), 0.5));
        }
        assert_eq!(changes, vec![Level::Normal, Level::Smooth]);
    }

    #[test]
    fn going_over_budget_steps_down() {
        let mut governor = Governor::new(Mode::Smooth);
        for _ in 1..OVER_BUDGET_SECONDS {
            assert_eq!(governor.tick(Some(5.0), 2.5), None);
        }
        assert_eq!(governor.tick(Some(5.0), 2.5), Some(Level::Normal));
        // Still over: down again.
        for _ in 1..OVER_BUDGET_SECONDS {
            let _ = governor.tick(Some(5.0), 2.5);
        }
        assert_eq!(governor.tick(Some(5.0), 2.5), Some(Level::Light));
        // Under budget but without headroom: stays put.
        for _ in 0..CALM_SECONDS * 2 {
            assert_eq!(governor.tick(Some(5.0), 1.5), None);
        }
        // With headroom it climbs back.
        let mut changes = Vec::new();
        for _ in 0..CALM_SECONDS * 2 {
            changes.extend(governor.tick(Some(5.0), 0.3));
        }
        assert_eq!(changes, vec![Level::Normal, Level::Smooth]);
    }

    #[test]
    fn changing_mode_resets() {
        let mut governor = Governor::new(Mode::Smooth);
        assert_eq!(governor.set_mode(Mode::Light), Some(Level::Light));
        assert_eq!(governor.set_mode(Mode::Light), None);
    }
}
