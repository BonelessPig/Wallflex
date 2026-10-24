//! Small decoded images (profile pictures, server icons).

/// A decoded image. Pixels are premultiplied `0xAARRGGBB`, rows top to
/// bottom.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Image {
    /// Width in pixels.
    pub width: usize,
    /// Height in pixels.
    pub height: usize,
    /// `width` x `height` pixels.
    pub pixels: Vec<u32>,
}

impl Image {
    /// An image from premultiplied BGRA bytes (WIC's `32bppPBGRA`).
    /// `None` if the byte count doesn't match the size.
    pub fn from_pbgra(
        width: usize,
        height: usize,
        bytes: &[u8],
    ) -> Option<Self> {
        let count = width.checked_mul(height)?;
        if bytes.len() != count.checked_mul(4)? {
            return None;
        }
        let (chunks, _) = bytes.as_chunks::<4>();
        let pixels = chunks.iter().map(|p| u32::from_le_bytes(*p)).collect();
        Some(Self {
            width,
            height,
            pixels,
        })
    }

    /// The pixel at (`x`, `y`) as premultiplied (r, g, b, a), each 0..=1,
    /// or transparent outside the image.
    pub fn pixel(&self, x: usize, y: usize) -> [f64; 4] {
        let index = y.checked_mul(self.width).and_then(|r| r.checked_add(x));
        let inside = x < self.width && y < self.height;
        let Some(&pixel) =
            index.filter(|_| inside).and_then(|i| self.pixels.get(i))
        else {
            return [0.0; 4];
        };
        let [blue, green, red, alpha] = pixel.to_le_bytes();
        [red, green, blue, alpha].map(|c| f64::from(c) / 255.0)
    }
}

#[cfg(test)]
mod tests {
    use super::Image;

    #[test]
    fn reads_pbgra() {
        let image = Image::from_pbgra(2, 1, &[0, 0, 255, 255, 10, 20, 30, 128])
            .unwrap();
        assert_eq!(image.pixel(0, 0), [1.0, 0.0, 0.0, 1.0]);
        assert_eq!(image.pixel(5, 0), [0.0; 4]);
        assert!(Image::from_pbgra(2, 2, &[0; 8]).is_none());
    }
}
