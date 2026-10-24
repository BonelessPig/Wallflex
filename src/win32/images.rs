//! Downloads and decodes small images (profile pictures, server icons)
//! on a background thread, using `WinHTTP` and Windows Imaging Component,
//! both built into Windows.

use std::collections::HashSet;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::Graphics::Imaging::{
    CLSID_WICImagingFactory, GUID_WICPixelFormat32bppPBGRA, IWICImagingFactory,
    WICBitmapDitherTypeNone, WICBitmapInterpolationModeFant,
    WICBitmapPaletteTypeCustom, WICDecodeMetadataCacheOnDemand, WICRect,
};
use windows::Win32::System::Com::IStream;
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED, CoCreateInstance,
    CoInitializeEx,
};
use windows::Win32::UI::WindowsAndMessaging::PostMessageW;
use windows::core::Interface;

use super::http;
use crate::discord::rpc::deadline;
use crate::image::Image;
use crate::log;

/// Images are decoded at this size (Discord serves 64 px avatars).
const DECODE_SIZE: u32 = 64;
/// Stack size for the fetcher thread.
const THREAD_STACK: usize = 256 * 1024;
/// Successful HTTP statuses.
const HTTP_OK: std::ops::Range<u32> = 200..300;
/// Waits before retrying a failed download (a flaky DNS lookup, say).
const RETRY_DELAYS: [Duration; 3] = [
    Duration::from_secs(5),
    Duration::from_secs(30),
    Duration::from_secs(120),
];
/// Hosts images may come from.
const ALLOWED_HOSTS: [&str; 2] = ["cdn.discordapp.com", "media.discordapp.net"];

/// Fetches images in the background and hands them to the UI.
pub struct ImageFetcher {
    /// URLs to fetch.
    requests: Sender<String>,
    /// Finished images waiting for the UI.
    ready: Arc<Mutex<Vec<(String, Image)>>>,
    /// URLs already asked for, so each is fetched once.
    asked: HashSet<String>,
}

impl ImageFetcher {
    /// Starts the fetcher thread. Each finished image is announced by
    /// posting `message` to `notify`.
    pub fn start(notify: HWND, message: u32) -> Self {
        let (requests, receiver) = mpsc::channel();
        let ready = Arc::new(Mutex::new(Vec::new()));
        let shared = Arc::clone(&ready);
        let notify = notify.0.expose_provenance();
        let spawned = thread::Builder::new()
            .name("images".into())
            .stack_size(THREAD_STACK)
            .spawn(move || fetch_loop(&receiver, &shared, notify, message));
        if let Err(error) = spawned {
            log!("can't start the image thread: {error}");
        }
        Self {
            requests,
            ready,
            asked: HashSet::new(),
        }
    }

    /// Asks for `url` unless it was asked for before.
    pub fn fetch(&mut self, url: &str) {
        if self.asked.insert(url.to_owned()) {
            // If the thread has gone, the picture just stays an initial.
            let _ = self.requests.send(url.to_owned());
        }
    }

    /// Takes the images finished since the last call.
    pub fn take_ready(&self) -> Vec<(String, Image)> {
        self.ready
            .lock()
            .map(|mut ready| std::mem::take(&mut *ready))
            .unwrap_or_default()
    }
}

/// The fetcher thread: fetch, decode, hand over, announce.
fn fetch_loop(
    requests: &Receiver<String>,
    ready: &Mutex<Vec<(String, Image)>>,
    notify: usize,
    message: u32,
) {
    // SAFETY: initialises COM for this thread, which WIC needs. A
    // failure shows up as failed decodes below.
    let _ = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
    // SAFETY: creates WIC's factory, an in-process COM object.
    let factory: Option<IWICImagingFactory> = unsafe {
        CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER)
    }
    .map_err(|e| log!("Windows Imaging Component unavailable: {e}"))
    .ok();
    let Some(factory) = factory else {
        return;
    };
    let mut retries: Vec<Retry> = Vec::new();
    while let Some(next) = next_job(requests, &mut retries) {
        let url = next.url.clone();
        match download(&url).and_then(|bytes| decode(&factory, &bytes)) {
            Ok(image) => {
                if let Ok(mut ready) = ready.lock() {
                    ready.push((url, image));
                }
                let hwnd = HWND(std::ptr::with_exposed_provenance_mut(notify));
                // SAFETY: posting is safe from any thread. If the window
                // is gone, nobody needs the picture.
                let _ = unsafe {
                    PostMessageW(Some(hwnd), message, WPARAM(0), LPARAM(0))
                };
            }
            Err(error) => match RETRY_DELAYS.get(next.attempt) {
                Some(&delay) => retries.push(Retry {
                    url,
                    attempt: next.attempt.saturating_add(1),
                    due: deadline(delay),
                }),
                None => log!("image {url}: {error}"),
            },
        }
    }
}

/// A download to try again later.
struct Retry {
    /// The image.
    url: String,
    /// Attempts made so far, minus one.
    attempt: usize,
    /// When to try again.
    due: Instant,
}

/// The next download: a retry that's due, or a new request (waiting
/// until one of those comes). `None` when Wallflex is closing.
fn next_job(
    requests: &Receiver<String>,
    retries: &mut Vec<Retry>,
) -> Option<Retry> {
    loop {
        let now = Instant::now();
        if let Some(index) = retries.iter().position(|r| r.due <= now) {
            return Some(retries.swap_remove(index));
        }
        let fresh = |url| Retry {
            url,
            attempt: 0,
            due: now,
        };
        match retries.iter().map(|r| r.due).min() {
            None => return requests.recv().ok().map(fresh),
            Some(due) => match requests
                .recv_timeout(due.saturating_duration_since(now))
            {
                Ok(url) => return Some(fresh(url)),
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => return None,
            },
        }
    }
}

/// Downloads an image from Discord's CDN.
fn download(url: &str) -> Result<Vec<u8>, String> {
    let rest = url.strip_prefix("https://").ok_or("not an https URL")?;
    let (host, path) = rest.split_once('/').ok_or("no path")?;
    if !ALLOWED_HOSTS.contains(&host) {
        return Err(format!("{host} isn't Discord's CDN"));
    }
    let response = http::request("GET", host, &format!("/{path}"), "", &[])?;
    if !HTTP_OK.contains(&response.status) {
        return Err(format!("HTTP {}", response.status));
    }
    Ok(response.body)
}

/// Decodes an image file (PNG, JPEG, WebP...) at [`DECODE_SIZE`].
fn decode(factory: &IWICImagingFactory, bytes: &[u8]) -> Result<Image, String> {
    // SAFETY: the stream reads `bytes`, which outlives it.
    let stream = unsafe {
        let stream = factory.CreateStream().map_err(|e| e.to_string())?;
        stream
            .InitializeFromMemory(bytes)
            .map_err(|e| e.to_string())?;
        stream
    };
    let stream: IStream = stream.cast().map_err(|e| e.to_string())?;
    decode_stream(factory, &stream, DECODE_SIZE)
}

/// Decodes the image in `stream` as a `size` x `size` square: scaled so
/// its shorter side fits, then cropped to the middle (so a wide video
/// thumbnail isn't squashed).
pub fn decode_stream(
    factory: &IWICImagingFactory,
    stream: &IStream,
    size: u32,
) -> Result<Image, String> {
    let edge = usize::try_from(size).map_err(|e| e.to_string())?;
    let stride = size.checked_mul(4).ok_or("size overflow")?;
    let mut pixels = vec![
        0_u8;
        edge.checked_mul(edge)
            .and_then(|n| n.checked_mul(4))
            .ok_or("size overflow")?
    ];
    // SAFETY: every object below is created from the factory and used on
    // this thread, and `pixels` holds exactly `stride` x `size` bytes
    // for CopyPixels.
    unsafe {
        let decoder = factory
            .CreateDecoderFromStream(
                stream,
                std::ptr::null(),
                WICDecodeMetadataCacheOnDemand,
            )
            .map_err(|e| e.to_string())?;
        let frame = decoder.GetFrame(0).map_err(|e| e.to_string())?;
        let (mut width, mut height) = (0_u32, 0_u32);
        frame
            .GetSize(&raw mut width, &raw mut height)
            .map_err(|e| e.to_string())?;
        let (scaled_w, scaled_h) = cover(width, height, size);
        let scaler = factory.CreateBitmapScaler().map_err(|e| e.to_string())?;
        scaler
            .Initialize(
                &frame,
                scaled_w,
                scaled_h,
                WICBitmapInterpolationModeFant,
            )
            .map_err(|e| e.to_string())?;
        let middle = WICRect {
            X: i32::try_from(scaled_w.saturating_sub(size) / 2).unwrap_or(0),
            Y: i32::try_from(scaled_h.saturating_sub(size) / 2).unwrap_or(0),
            Width: i32::try_from(size).map_err(|e| e.to_string())?,
            Height: i32::try_from(size).map_err(|e| e.to_string())?,
        };
        let clipper =
            factory.CreateBitmapClipper().map_err(|e| e.to_string())?;
        clipper
            .Initialize(&scaler, &raw const middle)
            .map_err(|e| e.to_string())?;
        let converter =
            factory.CreateFormatConverter().map_err(|e| e.to_string())?;
        converter
            .Initialize(
                &clipper,
                &GUID_WICPixelFormat32bppPBGRA,
                WICBitmapDitherTypeNone,
                None,
                0.0,
                WICBitmapPaletteTypeCustom,
            )
            .map_err(|e| e.to_string())?;
        converter
            .CopyPixels(std::ptr::null(), stride, &mut pixels)
            .map_err(|e| e.to_string())?;
    }
    Image::from_pbgra(edge, edge, &pixels)
        .ok_or_else(|| "bad pixel data".to_owned())
}

/// The size to scale a `width` x `height` image to so it covers a
/// `size` square (its shorter side becomes `size`).
fn cover(width: u32, height: u32, size: u32) -> (u32, u32) {
    let (w, h) = (u64::from(width.max(1)), u64::from(height.max(1)));
    let target = u64::from(size);
    let (w, h) = if w < h {
        (
            target,
            h.saturating_mul(target).checked_div(w).unwrap_or(target),
        )
    } else {
        (
            w.saturating_mul(target).checked_div(h).unwrap_or(target),
            target,
        )
    };
    (
        u32::try_from(w.max(target)).unwrap_or(size),
        u32::try_from(h.max(target)).unwrap_or(size),
    )
}
