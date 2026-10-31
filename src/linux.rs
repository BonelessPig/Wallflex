//! The layer for Linux (and other systems with no layer of their own).
//! Nothing is read yet: `docs/PORTING.md` lists where each reading
//! comes from (`/proc/stat`, `/proc/meminfo`, `/sys/class/net`, ...).

use crate::sensors::Snapshot;

/// Takes readings; for now every value is missing, which the widgets
/// already show as empty.
#[derive(Debug, Default)]
pub struct Sensors;

impl Sensors {
    /// Opens the sources (none yet).
    pub fn new() -> Self {
        Self
    }

    /// Takes one reading of everything.
    #[expect(
        clippy::unused_self,
        reason = "matches the other systems' Sensors, which keep state"
    )]
    pub fn sample(&mut self) -> Snapshot {
        Snapshot::default()
    }
}
