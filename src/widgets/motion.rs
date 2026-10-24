//! Smooth movement for the bars and rings: after each reading they glide
//! from where they were drawn to the new values over the glide time (set
//! by the performance level; none means they jump), easing
//! out, then stop. Text isn't animated; it shows the reading at once.

use std::time::Duration;

/// How long a glide takes unless told otherwise.
pub const GLIDE: Duration = Duration::from_millis(300);

/// How a glide moves over its time.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Ease {
    /// Fast at first, slowing into place.
    #[default]
    Out,
    /// An even speed. With a glide as long as the time between readings
    /// the bars never stop, so movement reads as continuous.
    Linear,
}

/// How a group of bars glides: for how long and how, or `None` to jump.
pub type Glide = Option<(Duration, Ease)>;

/// The animated values: how full each bar and ring is.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Levels {
    /// CPU thread load, in percent.
    pub cores: Vec<f64>,
    /// GPU ring fractions (0 to 1), five per GPU in ring order.
    pub rings: Vec<f64>,
    /// RAM in use, as a fraction (0 to 1).
    pub ram: f64,
}

impl Levels {
    /// `from` moved toward `to` by `amount` (0 = `from`, 1 = `to`). A list
    /// whose length changed (a GPU appeared) jumps straight to `to`.
    fn between(from: &Self, to: &Self, amount: f64) -> Self {
        let list = |a: &[f64], b: &[f64]| -> Vec<f64> {
            if a.len() == b.len() {
                a.iter().zip(b).map(|(a, b)| lerp(*a, *b, amount)).collect()
            } else {
                b.to_vec()
            }
        };
        Self {
            cores: list(&from.cores, &to.cores),
            rings: list(&from.rings, &to.rings),
            ram: lerp(from.ram, to.ram, amount),
        }
    }
}

/// A glide in progress, or a settled one.
#[derive(Debug)]
pub struct Motion {
    /// Where the glide started.
    from: Levels,
    /// How far along it is, 0 to 1 (1 = settled).
    progress: f64,
    /// How long a glide takes; `None` means bars jump.
    glide: Option<Duration>,
    /// How it moves over that time.
    ease: Ease,
}

impl Default for Motion {
    fn default() -> Self {
        Self {
            from: Levels::default(),
            progress: 1.0,
            glide: Some(GLIDE),
            ease: Ease::Out,
        }
    }
}

impl Motion {
    /// Starts a new glide from `shown`, what's on screen now.
    pub fn restart(&mut self, shown: Levels) {
        self.from = shown;
        self.progress = if self.glide.is_some() { 0.0 } else { 1.0 };
    }

    /// Sets how long glides take; `None` makes bars jump (and ends any
    /// glide under way).
    pub fn set_glide(&mut self, glide: Option<(Duration, Ease)>) {
        self.glide = glide.map(|(time, _)| time);
        self.ease = glide.map_or(Ease::Out, |(_, ease)| ease);
        if glide.is_none() {
            self.settle();
        }
    }

    /// Ends any glide: the next draw shows the target.
    pub fn settle(&mut self) {
        self.progress = 1.0;
    }

    /// Moves the glide on by `elapsed`. Returns whether it's still
    /// moving.
    pub fn advance(&mut self, elapsed: Duration) -> bool {
        let step = self
            .glide
            .filter(|g| !g.is_zero())
            .map_or(1.0, |g| elapsed.as_secs_f64() / g.as_secs_f64());
        self.progress = (self.progress + step).min(1.0);
        self.is_moving()
    }

    /// Whether a glide is under way.
    pub fn is_moving(&self) -> bool {
        self.progress < 1.0
    }

    /// What to draw now, on the way to `target`.
    pub fn shown(&self, target: &Levels) -> Levels {
        if self.is_moving() {
            let amount = match self.ease {
                Ease::Out => ease_out(self.progress),
                Ease::Linear => self.progress.clamp(0.0, 1.0),
            };
            Levels::between(&self.from, target, amount)
        } else {
            target.clone()
        }
    }
}

/// Fast at first, slowing into place (cubic ease-out).
fn ease_out(t: f64) -> f64 {
    let rest = 1.0 - t.clamp(0.0, 1.0);
    1.0 - rest * rest * rest
}

/// `a` moved toward `b` by `amount`.
fn lerp(a: f64, b: f64, amount: f64) -> f64 {
    a + (b - a) * amount
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::{Ease, GLIDE, Levels, Motion};

    #[test]
    fn linear_moves_evenly() {
        let mut motion = Motion::default();
        motion.set_glide(Some((GLIDE, Ease::Linear)));
        motion.restart(levels(0.0, 0.0));
        let _ = motion.advance(GLIDE / 4);
        let quarter = motion.shown(&levels(100.0, 1.0));
        assert!((quarter.cores[0] - 25.0).abs() < 1e-6);
    }

    fn levels(core: f64, ram: f64) -> Levels {
        Levels {
            cores: vec![core],
            rings: vec![core / 100.0],
            ram,
        }
    }

    #[test]
    fn settled_shows_the_target() {
        let motion = Motion::default();
        assert!(!motion.is_moving());
        assert_eq!(motion.shown(&levels(40.0, 0.5)), levels(40.0, 0.5));
    }

    #[test]
    fn glides_then_stops() {
        let mut motion = Motion::default();
        motion.restart(levels(0.0, 0.0));
        let target = levels(100.0, 1.0);
        assert_eq!(motion.shown(&target), levels(0.0, 0.0));
        assert!(motion.advance(GLIDE / 2));
        let half = motion.shown(&target);
        // Eased out: past halfway at half time.
        assert!(half.cores[0] > 50.0 && half.cores[0] < 100.0);
        assert!(half.ram > 0.5 && half.ram < 1.0);
        assert!(!motion.advance(GLIDE));
        assert_eq!(motion.shown(&target), target);
    }

    #[test]
    fn no_glide_jumps() {
        let mut motion = Motion::default();
        motion.set_glide(None);
        motion.restart(levels(0.0, 0.0));
        assert!(!motion.is_moving());
        assert_eq!(motion.shown(&levels(90.0, 1.0)), levels(90.0, 1.0));
    }

    #[test]
    fn new_lists_jump() {
        let mut motion = Motion::default();
        motion.restart(Levels::default());
        let _ = motion.advance(Duration::from_millis(1));
        let shown = motion.shown(&levels(80.0, 0.0));
        assert_eq!(shown.cores, vec![80.0]);
    }
}
