//! Colours.

/// An opaque colour, 8 bits per channel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rgb {
    /// Red, 0 to 255.
    pub r: u8,
    /// Green, 0 to 255.
    pub g: u8,
    /// Blue, 0 to 255.
    pub b: u8,
}

/// Number of hex digits in an `RRGGBB` colour.
const HEX_DIGITS: usize = 6;

/// Largest channel value, as a float, for converting to 0.0..=1.0.
const CHANNEL_MAX: f32 = 255.0;

impl Rgb {
    /// Black, the default background.
    pub const BLACK: Self = Self { r: 0, g: 0, b: 0 };

    /// Parses `RRGGBB` or `#RRGGBB` (case-insensitive).
    ///
    /// Returns `None` for anything else.
    pub fn from_hex(text: &str) -> Option<Self> {
        let digits = text.strip_prefix('#').unwrap_or(text);
        if digits.len() != HEX_DIGITS || !digits.is_ascii() {
            return None;
        }
        let channel = |range| {
            digits
                .get(range)
                .and_then(|pair| u8::from_str_radix(pair, 16).ok())
        };
        Some(Self {
            r: channel(0..2)?,
            g: channel(2..4)?,
            b: channel(4..6)?,
        })
    }

    /// The colour as a Win32 `COLORREF` value (`0x00BBGGRR`).
    pub fn to_colorref(self) -> u32 {
        u32::from_le_bytes([self.r, self.g, self.b, 0])
    }

    /// Red, green and blue scaled to 0.0..=1.0, as Direct2D expects.
    pub fn to_unit(self) -> [f32; 3] {
        [self.r, self.g, self.b].map(|c| f32::from(c) / CHANNEL_MAX)
    }
}

#[cfg(test)]
mod tests {
    use super::Rgb;

    #[test]
    fn parses_with_and_without_hash() {
        let expected = Rgb {
            r: 0x12,
            g: 0xAB,
            b: 0xFF,
        };
        assert_eq!(Rgb::from_hex("12abff"), Some(expected));
        assert_eq!(Rgb::from_hex("#12ABFF"), Some(expected));
    }

    #[test]
    fn rejects_bad_input() {
        for bad in ["", "#", "12345", "1234567", "12345g", "#éé1234"] {
            assert_eq!(Rgb::from_hex(bad), None, "{bad}");
        }
    }

    #[test]
    fn converts_to_colorref() {
        let c = Rgb {
            r: 0x11,
            g: 0x22,
            b: 0x33,
        };
        assert_eq!(c.to_colorref(), 0x0033_2211);
    }

    #[test]
    fn converts_to_unit_floats() {
        let [r, g, b] = Rgb {
            r: 255,
            g: 0,
            b: 51,
        }
        .to_unit();
        assert!((r - 1.0).abs() < f32::EPSILON);
        assert!(g.abs() < f32::EPSILON);
        assert!((b - 0.2).abs() < 1e-6);
    }
}
