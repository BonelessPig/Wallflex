//! The app icon: three bars, like the CPU columns, on a blue rounded
//! square. It is drawn here with the canvas and saved as `wallflex.ico`,
//! which `build.rs` embeds in the exe (Explorer, the Start menu and the
//! tray all use it). Only tests build this module: a test checks the
//! committed file still matches the drawing, and rewrites it when
//! `WALLFLEX_WRITE_ICON` is set.

use crate::canvas::{Area, Canvas, Paint};
use crate::color::Rgb;

/// The sizes in the icon file, in pixels. 16, 20, 24, 32, 40 and 48 are
/// the small icon at 100% to 300% scaling (the tray picks one); the
/// rest are for Explorer and the Start menu. No 256: as an uncompressed
/// bitmap it would add 270 KB to the exe, and Windows scales up 128.
pub const SIZES: [u32; 9] = [16, 20, 24, 32, 40, 48, 64, 96, 128];

/// The background (the widgets' blue).
const BLUE: Rgb = Rgb {
    r: 0xA8,
    g: 0xE6,
    b: 0xFF,
};
/// The bars.
const DARK: Rgb = Rgb {
    r: 0x10,
    g: 0x14,
    b: 0x18,
};
/// Corner radius, as a fraction of the edge.
const CORNER: f64 = 0.22;
/// Bar width, as a fraction of the edge.
const BAR: f64 = 0.16;
/// Gap between bars, as a fraction of the edge.
const GAP: f64 = 0.06;
/// Left edge of the first bar, as a fraction of the edge.
const LEFT: f64 = 0.22;
/// Where the bars stand, as a fraction of the edge from the top.
const BASE: f64 = 0.82;
/// The bars' heights, as fractions of the edge.
const HEIGHTS: [f64; 3] = [0.45, 0.72, 0.58];

/// Bytes in an `ICONDIR` header.
const DIR_BYTES: u32 = 6;
/// Bytes in each `ICONDIRENTRY`.
const ENTRY_BYTES: u32 = 16;
/// Bytes in a `BITMAPINFOHEADER`.
const INFO_BYTES: u32 = 40;
/// Bytes per pixel.
const PIXEL_BYTES: u32 = 4;
/// Bits per pixel.
const PIXEL_BITS: u16 = 32;
/// `ICONDIR` type for icons (2 is cursors).
const TYPE_ICON: u16 = 1;
/// Mask rows are padded to a multiple of 32 bits.
const MASK_ROW_BITS: u32 = 32;
/// Bytes in each 32-bit unit of a mask row.
const MASK_UNIT_BYTES: u32 = 4;

/// Draws the icon at `edge` x `edge` pixels, as straight-alpha
/// `0xAARRGGBB` pixels, rows top to bottom.
pub fn draw(edge: u32) -> Option<Vec<u32>> {
    let side = usize::try_from(edge).ok()?;
    let count = side.checked_mul(side)?;
    let s = f64::from(edge);
    let half = s / 2.0;
    // The shape's coverage, for the alpha channel.
    let mut mask = vec![0_u32; count];
    let white = Paint {
        color: Rgb {
            r: 255,
            g: 255,
            b: 255,
        },
        alpha: 1.0,
    };
    Canvas::new(side, side, &mut mask)?.fill_rounded(
        (half, half),
        half,
        s * CORNER,
        white,
    );
    // The picture itself.
    let mut art = vec![0_u32; count];
    {
        let mut canvas = Canvas::new(side, side, &mut art)?;
        canvas.fill(BLUE);
        let dark = Paint {
            color: DARK,
            alpha: 1.0,
        };
        for (index, height) in (0_u32..).zip(HEIGHTS) {
            let x0 = s * LEFT + f64::from(index) * s * (BAR + GAP);
            let area = Area {
                x0,
                y0: s * (BASE - height),
                x1: x0 + s * BAR,
                y1: s * BASE,
            };
            canvas.fill_rect(area, dark);
        }
    }
    let pixels = art
        .iter()
        .zip(&mask)
        .map(|(colour, coverage)| {
            let [b, g, r, _] = colour.to_le_bytes();
            let [alpha, _, _, _] = coverage.to_le_bytes();
            u32::from_le_bytes([b, g, r, alpha])
        })
        .collect();
    Some(pixels)
}

/// The icon file: every size in `SIZES` as a 32-bit bitmap.
pub fn ico() -> Option<Vec<u8>> {
    let mut images = Vec::new();
    for edge in SIZES {
        images.push((edge, image(edge, &draw(edge)?)?));
    }
    let count = u16::try_from(images.len()).ok()?;
    let mut out = Vec::new();
    out.extend_from_slice(&0_u16.to_le_bytes()); // reserved
    out.extend_from_slice(&TYPE_ICON.to_le_bytes());
    out.extend_from_slice(&count.to_le_bytes());
    let mut offset = ENTRY_BYTES
        .checked_mul(u32::from(count))?
        .checked_add(DIR_BYTES)?;
    for (edge, bytes) in &images {
        let length = u32::try_from(bytes.len()).ok()?;
        let side = u8::try_from(*edge).ok()?;
        out.extend_from_slice(&[side, side, 0, 0]); // no palette
        out.extend_from_slice(&1_u16.to_le_bytes()); // planes
        out.extend_from_slice(&PIXEL_BITS.to_le_bytes());
        out.extend_from_slice(&length.to_le_bytes());
        out.extend_from_slice(&offset.to_le_bytes());
        offset = offset.checked_add(length)?;
    }
    for (_, bytes) in &images {
        out.extend_from_slice(bytes);
    }
    Some(out)
}

/// One icon image: a `BITMAPINFOHEADER` (twice the height, for the
/// colour and mask halves), the pixels bottom row first, then an
/// all-clear mask, since the alpha channel decides.
fn image(edge: u32, pixels: &[u32]) -> Option<Vec<u8>> {
    let side = usize::try_from(edge).ok()?;
    let mask_row = edge.div_ceil(MASK_ROW_BITS).checked_mul(MASK_UNIT_BYTES)?;
    let colour_bytes = edge.checked_mul(edge)?.checked_mul(PIXEL_BYTES)?;
    let mask_bytes = mask_row.checked_mul(edge)?;
    let image_bytes = colour_bytes.checked_add(mask_bytes)?;
    let width = i32::try_from(edge).ok()?;
    let height = width.checked_mul(2)?;

    let mut out = Vec::new();
    out.extend_from_slice(&INFO_BYTES.to_le_bytes());
    out.extend_from_slice(&width.to_le_bytes());
    out.extend_from_slice(&height.to_le_bytes());
    out.extend_from_slice(&1_u16.to_le_bytes()); // planes
    out.extend_from_slice(&PIXEL_BITS.to_le_bytes());
    out.extend_from_slice(&0_u32.to_le_bytes()); // BI_RGB
    out.extend_from_slice(&image_bytes.to_le_bytes());
    out.extend_from_slice(&[0; 16]); // resolution, palette: unused
    for row in pixels.chunks(side).rev() {
        for pixel in row {
            out.extend_from_slice(&pixel.to_le_bytes());
        }
    }
    out.resize(out.len().checked_add(usize::try_from(mask_bytes).ok()?)?, 0);
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::{SIZES, draw, ico};

    /// The committed icon, next to `Cargo.toml`.
    const ICON_FILE: &str =
        concat!(env!("CARGO_MANIFEST_DIR"), "/wallflex.ico");

    #[test]
    fn committed_icon_matches_the_drawing() {
        let fresh = ico().unwrap();
        if std::env::var_os("WALLFLEX_WRITE_ICON").is_some() {
            std::fs::write(ICON_FILE, &fresh).unwrap();
        }
        let committed = std::fs::read(ICON_FILE).unwrap_or_default();
        assert!(
            committed == fresh,
            "wallflex.ico is out of date: run \
             `WALLFLEX_WRITE_ICON=1 cargo test icon` and commit it"
        );
    }

    #[test]
    fn corners_are_clear_and_the_middle_is_solid() {
        let pixels = draw(32).unwrap();
        let alpha = |index: usize| pixels[index].to_le_bytes()[3];
        assert_eq!(alpha(0), 0);
        assert_eq!(alpha(16 * 32 + 2), 255);
    }

    #[test]
    fn directory_lists_every_size_in_order() {
        let file = ico().unwrap();
        assert_eq!(&file[..6], &[0, 0, 1, 0, 9, 0]);
        let first = 6 + 16 * SIZES.len();
        let mut offset = first;
        for (index, edge) in SIZES.iter().enumerate() {
            let entry = &file[6 + 16 * index..][..16];
            assert_eq!(u32::from(entry[0]), *edge);
            let length = u32::from_le_bytes(entry[8..12].try_into().unwrap());
            let start = u32::from_le_bytes(entry[12..16].try_into().unwrap());
            assert_eq!(usize::try_from(start).unwrap(), offset);
            offset += usize::try_from(length).unwrap();
        }
        assert_eq!(offset, file.len());
    }
}
