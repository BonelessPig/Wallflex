//! Writes 32-bit BMP files, for `--preview`.

/// Size of the file header plus the `BITMAPINFOHEADER`.
const HEADER_BYTES: u32 = 14 + 40;
/// Size of a `BITMAPINFOHEADER`.
const INFO_BYTES: u32 = 40;
/// Bytes per pixel.
const PIXEL_BYTES: u32 = 4;
/// Bits per pixel.
const PIXEL_BITS: u16 = 32;
/// Pixels per metre at 96 DPI, for the header's resolution fields.
const PIXELS_PER_METRE: i32 = 3_780;

/// Encodes `0x00RRGGBB` pixels (rows top to bottom) as a BMP file.
/// `None` if the size doesn't fit a BMP or doesn't match `pixels`.
pub fn encode(width: u32, height: u32, pixels: &[u32]) -> Option<Vec<u8>> {
    let count = usize::try_from(width.checked_mul(height)?).ok()?;
    if pixels.len() != count {
        return None;
    }
    let image_bytes = width.checked_mul(height)?.checked_mul(PIXEL_BYTES)?;
    let file_bytes = image_bytes.checked_add(HEADER_BYTES)?;
    let width_i = i32::try_from(width).ok()?;
    // Negative height: rows stored top to bottom.
    let height_i = i32::try_from(height).ok()?.checked_neg()?;

    let mut out = Vec::with_capacity(usize::try_from(file_bytes).ok()?);
    out.extend_from_slice(b"BM");
    out.extend_from_slice(&file_bytes.to_le_bytes());
    out.extend_from_slice(&0_u32.to_le_bytes()); // reserved
    out.extend_from_slice(&HEADER_BYTES.to_le_bytes()); // pixel offset
    out.extend_from_slice(&INFO_BYTES.to_le_bytes());
    out.extend_from_slice(&width_i.to_le_bytes());
    out.extend_from_slice(&height_i.to_le_bytes());
    out.extend_from_slice(&1_u16.to_le_bytes()); // planes
    out.extend_from_slice(&PIXEL_BITS.to_le_bytes());
    out.extend_from_slice(&0_u32.to_le_bytes()); // BI_RGB
    out.extend_from_slice(&image_bytes.to_le_bytes());
    out.extend_from_slice(&PIXELS_PER_METRE.to_le_bytes());
    out.extend_from_slice(&PIXELS_PER_METRE.to_le_bytes());
    out.extend_from_slice(&0_u32.to_le_bytes()); // palette size
    out.extend_from_slice(&0_u32.to_le_bytes()); // important colours
    for pixel in pixels {
        out.extend_from_slice(&pixel.to_le_bytes());
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::encode;

    #[test]
    fn encodes_header_and_pixels() {
        let file = encode(2, 1, &[0x00FF_0000, 0x0000_00FF]);
        let file = file.unwrap_or_default();
        assert_eq!(file.len(), 54 + 8);
        assert_eq!(file.get(..2), Some(b"BM".as_slice()));
        assert_eq!(file.get(18..22), Some(2_i32.to_le_bytes().as_slice()));
        assert_eq!(file.get(22..26), Some((-1_i32).to_le_bytes().as_slice()));
        // First pixel, red: stored blue, green, red, 0.
        assert_eq!(file.get(54..58), Some([0, 0, 255, 0].as_slice()));
    }

    #[test]
    fn rejects_mismatched_sizes() {
        assert_eq!(encode(2, 2, &[0; 3]), None);
    }
}
