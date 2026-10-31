//! A monitor's place and size, as each system's layer reports it.

use crate::geometry::Rect;

/// One monitor, in desktop coordinates (physical pixels). The primary
/// monitor's top-left is (0, 0); others can be negative.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Monitor {
    /// The whole monitor.
    pub rect: Rect,
    /// The part the taskbar and docked toolbars leave free.
    pub work: Rect,
    /// The system's name for it, e.g. `\.\DISPLAY1` on Windows.
    pub name: String,
    /// Whether it's the primary monitor.
    pub primary: bool,
    /// The screen's width and height in millimetres, if the monitor
    /// says.
    pub size_mm: Option<(u32, u32)>,
}

impl Monitor {
    /// Millimetres per pixel across, if the size is known.
    pub fn pitch(&self) -> Option<f64> {
        let (width_mm, _) = self.size_mm?;
        let pixels = self.rect.width();
        (pixels > 0).then(|| f64::from(width_mm) / f64::from(pixels))
    }
}
