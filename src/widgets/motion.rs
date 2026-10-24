//! Smooth movement for the bars and rings: after each reading they glide
//! from where they were drawn to the new values over [`GLIDE`], easing
//! out, then stop. Text isn't animated; it shows the reading at once.

use std::time::Duration;

/// How long a glide to a new reading takes.
pub const GLIDE: Duration = Duration::from_millis(300);

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
}

impl Default for Motion {
    fn default() -> Self {
        Self {
            from: Levels::default(),
            progress: 1.0,
        }
    }
}

impl Motion {
    /// Starts a new glide from `shown`, what's on screen now.
    pub fn restart(&mut self, shown: Levels) {
        self.from = shown;
        self.progress = 0.0;
    }

    /// Ends any glide: the next draw shows the target.
    pub fn settle(&mut self) {
        self.progress = 1.0;
    }

    /// Moves the glide on by `elapsed`. Returns whether it's still
    /// moving.
    pub fn advance(&mut self, elapsed: Duration) -> bool {
        let step = elapsed.as_secs_f64() / GLIDE.as_secs_f64();
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
            Levels::between(&self.from, target, ease_out(self.progress))
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

    use super::{GLIDE, Levels, Motion};

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
    fn new_lists_jump() {
        let mut motion = Motion::default();
        motion.restart(Levels::default());
        let _ = motion.advance(Duration::from_millis(1));
        let shown = motion.shown(&levels(80.0, 0.0));
        assert_eq!(shown.cores, vec![80.0]);
    }
}
