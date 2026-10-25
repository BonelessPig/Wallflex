//! Reads a picture file and paints it, scaled, onto a window. The
//! picture is decoded and scaled by Windows Imaging Component a band of
//! rows at a time, so no copy the size of the monitor is ever held:
//! once painted, the window keeps the pixels and the memory is gone.

use std::path::Path;

use windows::Win32::Foundation::{GENERIC_READ, RPC_E_CHANGED_MODE};
use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, DIB_RGB_COLORS, HDC, SRCCOPY,
    StretchDIBits,
};
use windows::Win32::Graphics::Imaging::{
    CLSID_WICImagingFactory, GUID_WICPixelFormat32bppPBGRA, IWICBitmapSource,
    IWICImagingFactory, WICBitmapDitherTypeNone,
    WICBitmapInterpolationModeHighQualityCubic, WICBitmapPaletteTypeCustom,
    WICDecodeMetadataCacheOnDemand, WICRect,
};
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance,
    CoInitializeEx, CoUninitialize,
};
use windows::core::{HSTRING, Interface};

use super::struct_size;
use crate::backdrop::Mapping;
use crate::color::Rgb;

/// Most bytes of picture held at once while painting.
const BAND_BYTES: usize = 1024 * 1024;
/// Bytes per pixel (BGRA).
const PIXEL_BYTES: usize = 4;
/// Bits per pixel of the bands.
const BITS_PER_PIXEL: u16 = 32;

/// An opened picture file: its size is known, its pixels are read only
/// while painting.
pub struct Picture {
    /// WIC's factory.
    factory: IWICImagingFactory,
    /// The first frame, as premultiplied BGRA.
    source: IWICBitmapSource,
    /// Width and height in pixels.
    size: (u32, u32),
    /// Keeps COM initialised; dropped last, after the objects above.
    _com: Com,
}

impl Picture {
    /// Opens the picture at `path` (any format Windows reads: JPEG, PNG,
    /// BMP, GIF, TIFF, and WebP or HEIC where installed).
    ///
    /// # Errors
    ///
    /// A message saying why it can't be read.
    pub fn open(path: &Path) -> Result<Self, String> {
        let com = Com::init()?;
        // SAFETY: creates WIC's factory, an in-process COM object, on
        // this thread, where COM is initialised for as long as `com`
        // lives (it's kept with the objects).
        let factory: IWICImagingFactory = unsafe {
            CoCreateInstance(
                &CLSID_WICImagingFactory,
                None,
                CLSCTX_INPROC_SERVER,
            )
        }
        .map_err(|e| e.to_string())?;
        let wide = HSTRING::from(path.as_os_str());
        // SAFETY: the objects are created from the factory and used only
        // on this thread; `wide` outlives the call.
        let (source, size) = unsafe {
            let decoder = factory
                .CreateDecoderFromFilename(
                    &wide,
                    None,
                    GENERIC_READ,
                    WICDecodeMetadataCacheOnDemand,
                )
                .map_err(|e| e.to_string())?;
            let frame = decoder.GetFrame(0).map_err(|e| e.to_string())?;
            let (mut width, mut height) = (0_u32, 0_u32);
            frame
                .GetSize(&raw mut width, &raw mut height)
                .map_err(|e| e.to_string())?;
            let converter =
                factory.CreateFormatConverter().map_err(|e| e.to_string())?;
            converter
                .Initialize(
                    &frame,
                    &GUID_WICPixelFormat32bppPBGRA,
                    WICBitmapDitherTypeNone,
                    None,
                    0.0,
                    WICBitmapPaletteTypeCustom,
                )
                .map_err(|e| e.to_string())?;
            let source = converter
                .cast::<IWICBitmapSource>()
                .map_err(|e| e.to_string())?;
            (source, (width, height))
        };
        Ok(Self {
            factory,
            source,
            size,
            _com: com,
        })
    }

    /// Width and height in pixels.
    pub fn size(&self) -> (u32, u32) {
        self.size
    }

    /// Scales the picture to `mapping.scaled` and paints its
    /// `mapping.source` part at `mapping.dest` on `hdc`. See-through
    /// parts of the picture show `matte`.
    ///
    /// # Errors
    ///
    /// A message saying why it couldn't be drawn.
    pub fn paint(
        &self,
        hdc: HDC,
        mapping: Mapping,
        matte: Rgb,
    ) -> Result<(), String> {
        // SAFETY: as in `open`.
        let scaled = unsafe {
            let scaler = self
                .factory
                .CreateBitmapScaler()
                .map_err(|e| e.to_string())?;
            scaler
                .Initialize(
                    &self.source,
                    mapping.scaled.0,
                    mapping.scaled.1,
                    WICBitmapInterpolationModeHighQualityCubic,
                )
                .map_err(|e| e.to_string())?;
            scaler
        };
        paint_bands(
            hdc,
            &scaled.cast().map_err(|e| e.to_string())?,
            mapping,
            matte,
        )
    }
}

/// Copies `mapping.source` of `scaled` to `mapping.dest` on `hdc`, a
/// band of rows at a time.
fn paint_bands(
    hdc: HDC,
    scaled: &IWICBitmapSource,
    mapping: Mapping,
    matte: Rgb,
) -> Result<(), String> {
    let Mapping { source, dest, .. } = mapping;
    let width = usize::try_from(source.width()).map_err(|e| e.to_string())?;
    let stride = width.checked_mul(PIXEL_BYTES).ok_or("too wide")?;
    let band_rows = BAND_BYTES.checked_div(stride).unwrap_or(1).max(1);
    let band_rows_i32 = i32::try_from(band_rows).unwrap_or(i32::MAX);
    let stride_u32 = u32::try_from(stride).map_err(|e| e.to_string())?;
    let mut band = vec![0_u8; stride.checked_mul(band_rows).ok_or("too big")?];
    let mut top = source.top;
    while top < source.bottom {
        let rows = band_rows_i32.min(source.bottom.saturating_sub(top));
        let rows_usize = usize::try_from(rows).map_err(|e| e.to_string())?;
        let bytes = stride.checked_mul(rows_usize).ok_or("too big")?;
        let pixels = band.get_mut(..bytes).ok_or("band too small")?;
        let area = WICRect {
            X: source.left,
            Y: top,
            Width: source.width(),
            Height: rows,
        };
        // SAFETY: `pixels` holds exactly `stride` x `rows` bytes, which
        // is what CopyPixels writes for `area`.
        unsafe { scaled.CopyPixels(&raw const area, stride_u32, pixels) }
            .map_err(|e| e.to_string())?;
        flatten(pixels, matte);
        let y = dest.top.saturating_add(top.saturating_sub(source.top));
        blit(hdc, (dest.left, y), (source.width(), rows), pixels)?;
        top = top.saturating_add(rows);
    }
    Ok(())
}

/// Lays premultiplied BGRA `pixels` over `matte`, making them opaque.
fn flatten(pixels: &mut [u8], matte: Rgb) {
    let (chunks, _) = pixels.as_chunks_mut::<PIXEL_BYTES>();
    for pixel in chunks {
        let [b, g, r, a] = *pixel;
        if a == u8::MAX {
            continue;
        }
        let keep = u16::from(u8::MAX.saturating_sub(a));
        let under = |c: u8, m: u8| {
            let add = u16::from(m)
                .saturating_mul(keep)
                .checked_div(u16::from(u8::MAX))
                .unwrap_or(0);
            u8::try_from(u16::from(c).saturating_add(add)).unwrap_or(u8::MAX)
        };
        *pixel = [
            under(b, matte.b),
            under(g, matte.g),
            under(r, matte.r),
            u8::MAX,
        ];
    }
}

/// Draws `size` (width, height) BGRA `pixels`, rows top to bottom, at
/// `at` on `hdc`.
fn blit(
    hdc: HDC,
    at: (i32, i32),
    size: (i32, i32),
    pixels: &[u8],
) -> Result<(), String> {
    let info = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: struct_size::<BITMAPINFOHEADER>(),
            biWidth: size.0,
            // Negative: rows run top to bottom.
            biHeight: size.1.checked_neg().ok_or("too tall")?,
            biPlanes: 1,
            biBitCount: BITS_PER_PIXEL,
            biCompression: BI_RGB.0,
            ..Default::default()
        },
        ..Default::default()
    };
    // SAFETY: `info` describes `pixels` exactly (checked by the caller's
    // band arithmetic), and both outlive the call.
    let lines = unsafe {
        StretchDIBits(
            hdc,
            at.0,
            at.1,
            size.0,
            size.1,
            0,
            0,
            size.0,
            size.1,
            Some(pixels.as_ptr().cast()),
            &raw const info,
            DIB_RGB_COLORS,
            SRCCOPY,
        )
    };
    if lines == 0 {
        return Err("couldn't draw the picture".into());
    }
    Ok(())
}

/// COM, initialised on this thread for as long as it lives.
struct Com {
    /// Whether this initialisation must be undone.
    owned: bool,
}

impl Com {
    /// Initialises COM on this thread, unless it already is (in any
    /// mode, which is fine for WIC).
    fn init() -> Result<Self, String> {
        // SAFETY: plain call; paired with CoUninitialize on drop when it
        // succeeds.
        let result = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
        if result.is_ok() {
            return Ok(Self { owned: true });
        }
        if result == RPC_E_CHANGED_MODE {
            return Ok(Self { owned: false });
        }
        Err(windows::core::Error::from(result).to_string())
    }
}

impl Drop for Com {
    fn drop(&mut self) {
        if self.owned {
            // SAFETY: undoes this thread's successful CoInitializeEx.
            unsafe { CoUninitialize() };
        }
    }
}
