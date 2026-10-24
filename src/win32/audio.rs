//! Captures what's playing on the default speakers ("loopback"), with
//! Windows' own audio API (WASAPI): no admin, no driver. A thread waits
//! for Windows to hand over sound, mixes it to mono into
//! [`Recent`](crate::audio::Recent), and tells the UI when sound starts
//! after silence. While nothing plays, the thread just sleeps.

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use windows::Win32::Foundation::{
    CloseHandle, HANDLE, HWND, LPARAM, WAIT_OBJECT_0, WPARAM,
};
use windows::Win32::Media::Audio::{
    AUDCLNT_BUFFERFLAGS_SILENT, AUDCLNT_SHAREMODE_SHARED,
    AUDCLNT_STREAMFLAGS_EVENTCALLBACK, AUDCLNT_STREAMFLAGS_LOOPBACK,
    IAudioCaptureClient, IAudioClient, IMMDevice, IMMDeviceEnumerator,
    MMDeviceEnumerator, WAVE_FORMAT_PCM, WAVEFORMATEX, WAVEFORMATEXTENSIBLE,
    eConsole, eRender,
};
use windows::Win32::Media::KernelStreaming::{
    KSDATAFORMAT_SUBTYPE_PCM, WAVE_FORMAT_EXTENSIBLE,
};
use windows::Win32::Media::Multimedia::{
    KSDATAFORMAT_SUBTYPE_IEEE_FLOAT, WAVE_FORMAT_IEEE_FLOAT,
};
use windows::Win32::System::Com::{
    CLSCTX_ALL, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx,
    CoTaskMemFree,
};
use windows::Win32::System::Threading::{CreateEventW, WaitForSingleObject};
use windows::Win32::UI::WindowsAndMessaging::PostMessageW;

use crate::audio::{Format, Recent, SILENCE, SampleKind};
use crate::log;

/// Stack size for the capture thread.
const THREAD_STACK: usize = 256 * 1024;
/// Windows' capture buffer: 200 ms, in 100 ns units.
const BUFFER_100NS: i64 = 2_000_000;
/// How long to wait for sound before checking for a stop or a new
/// default device, in milliseconds.
const WAIT_MS: u32 = 250;
/// How often to check whether the default speakers changed.
const DEVICE_CHECK: Duration = Duration::from_secs(5);
/// Quiet for this long counts as silence.
const QUIET_AFTER: Duration = Duration::from_millis(500);
/// Wait before trying again when the speakers can't be opened.
const RETRY: Duration = Duration::from_secs(3);
/// Steps of the retry wait, so a stop is noticed quickly.
const RETRY_STEP: Duration = Duration::from_millis(250);

/// What the capture thread shares with the UI.
#[derive(Default)]
struct Shared {
    /// The latest samples.
    recent: Mutex<Recent>,
    /// Their sample rate, in Hz.
    rate: AtomicU32,
    /// Whether sound is playing.
    loud: AtomicBool,
    /// Set to stop the thread.
    stop: AtomicBool,
}

/// The running capture; stopped on drop.
pub struct AudioTap {
    /// Shared with the thread.
    shared: Arc<Shared>,
    /// The thread, joined on drop.
    thread: Option<JoinHandle<()>>,
}

impl AudioTap {
    /// Starts capturing. When sound starts after silence, `message` is
    /// posted to `notify`.
    pub fn start(notify: HWND, message: u32) -> Self {
        let shared = Arc::new(Shared::default());
        let theirs = Arc::clone(&shared);
        let notify = notify.0.expose_provenance();
        let thread = thread::Builder::new()
            .name("audio".into())
            .stack_size(THREAD_STACK)
            .spawn(move || run(&theirs, notify, message));
        let thread = match thread {
            Ok(thread) => Some(thread),
            Err(error) => {
                log!("can't start the audio thread: {error}");
                None
            }
        };
        Self { shared, thread }
    }

    /// Copies the latest samples into `out` and returns their rate.
    pub fn latest(&self, out: &mut Vec<f32>) -> u32 {
        if let Ok(recent) = self.shared.recent.lock() {
            recent.copy_to(out);
        }
        self.shared.rate.load(Ordering::Acquire)
    }

    /// Whether sound is playing.
    pub fn is_loud(&self) -> bool {
        self.shared.loud.load(Ordering::Acquire)
    }
}

impl Drop for AudioTap {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::Release);
        if let Some(thread) = self.thread.take() {
            // The thread notices within WAIT_MS; a panic there has
            // already been reported.
            let _ = thread.join();
        }
    }
}

/// The thread: captures until stopped, reopening the speakers when they
/// change or fail.
fn run(shared: &Shared, notify: usize, message: u32) {
    // SAFETY: once per thread, before any COM call; an "already
    // initialised" result is fine.
    let _ = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
    let mut last_error = String::new();
    while !shared.stop.load(Ordering::Acquire) {
        match capture(shared, notify, message) {
            Ok(()) => last_error.clear(),
            Err(error) => {
                // Log each new problem once, not every retry.
                if error != last_error {
                    log!("audio: {error}");
                    last_error = error;
                }
                shared.loud.store(false, Ordering::Release);
                let until = Instant::now().checked_add(RETRY);
                while until.is_some_and(|u| Instant::now() < u)
                    && !shared.stop.load(Ordering::Acquire)
                {
                    thread::sleep(RETRY_STEP);
                }
            }
        }
    }
}

/// An event handle, closed on drop.
struct Event(HANDLE);

impl Drop for Event {
    fn drop(&mut self) {
        // SAFETY: created by `capture`, closed only here.
        let _ = unsafe { CloseHandle(self.0) };
    }
}

/// Captures from the default speakers until stopped (`Ok`) or until they
/// change or fail (`Err`, to reopen).
fn capture(shared: &Shared, notify: usize, message: u32) -> Result<(), String> {
    let text = |e: windows::core::Error| e.message();
    // SAFETY: COM is initialised on this thread; plain arguments.
    let enumerator: IMMDeviceEnumerator =
        unsafe { CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL) }
            .map_err(text)?;
    // SAFETY: a valid enumerator.
    let device =
        unsafe { enumerator.GetDefaultAudioEndpoint(eRender, eConsole) }
            .map_err(|e| format!("no speakers ({})", e.message()))?;
    let id = device_id(&device);
    // SAFETY: a valid device; no activation parameters.
    let client: IAudioClient =
        unsafe { device.Activate(CLSCTX_ALL, None) }.map_err(text)?;
    let (format, rate) = open_stream(&client)?;
    shared.rate.store(rate, Ordering::Release);
    // SAFETY: an auto-reset event, closed by `Event`.
    let event =
        Event(unsafe { CreateEventW(None, false, false, None) }.map_err(text)?);
    // SAFETY: the client and event are valid.
    unsafe { client.SetEventHandle(event.0) }.map_err(text)?;
    // SAFETY: the stream is initialised.
    let reader: IAudioCaptureClient =
        unsafe { client.GetService() }.map_err(text)?;
    // SAFETY: as above.
    unsafe { client.Start() }.map_err(text)?;
    log!(
        "audio: listening ({} Hz, {} channels)",
        shared.rate.load(Ordering::Acquire),
        format.channels
    );
    let result = listen(
        shared,
        &reader,
        &event,
        (notify, message),
        format,
        &enumerator,
        &id,
    );
    // SAFETY: stops the stream started above; nothing to do on failure.
    let _ = unsafe { client.Stop() };
    result
}

/// Sets up a loopback stream in the mixer's format. Returns the sample
/// format and rate (Hz).
fn open_stream(client: &IAudioClient) -> Result<(Format, u32), String> {
    // SAFETY: a valid client; the format is freed below.
    let mix = unsafe { client.GetMixFormat() }.map_err(|e| e.message())?;
    let parsed = read_format(mix);
    // SAFETY: shared-mode loopback in the mixer's own format, which
    // Windows always accepts; `mix` is valid until freed below.
    let opened = unsafe {
        client.Initialize(
            AUDCLNT_SHAREMODE_SHARED,
            AUDCLNT_STREAMFLAGS_LOOPBACK | AUDCLNT_STREAMFLAGS_EVENTCALLBACK,
            BUFFER_100NS,
            0,
            mix,
            None,
        )
    };
    // SAFETY: allocated by GetMixFormat with CoTaskMemAlloc.
    unsafe { CoTaskMemFree(Some(mix.cast_const().cast())) };
    opened.map_err(|e| e.message())?;
    parsed.ok_or_else(|| "unsupported audio format".to_owned())
}

/// The sample format and rate (Hz) of `mix`; `None` for formats the
/// visualizer can't read.
fn read_format(mix: *const WAVEFORMATEX) -> Option<(Format, u32)> {
    // SAFETY: `mix` points to a WAVEFORMATEX from GetMixFormat (packed,
    // so read unaligned).
    let base = unsafe { mix.read_unaligned() };
    let tag = u32::from(base.wFormatTag);
    let channels = usize::from(base.nChannels);
    let frame = usize::from(base.nBlockAlign);
    let container_bits = frame
        .checked_div(channels)
        .and_then(|bytes| bytes.checked_mul(8))?;
    let float = if tag == WAVE_FORMAT_EXTENSIBLE {
        // SAFETY: the tag says the larger structure is there.
        let ext =
            unsafe { mix.cast::<WAVEFORMATEXTENSIBLE>().read_unaligned() };
        let sub = ext.SubFormat;
        if sub == KSDATAFORMAT_SUBTYPE_IEEE_FLOAT {
            true
        } else if sub == KSDATAFORMAT_SUBTYPE_PCM {
            false
        } else {
            return None;
        }
    } else if tag == WAVE_FORMAT_IEEE_FLOAT {
        true
    } else if tag == WAVE_FORMAT_PCM {
        false
    } else {
        return None;
    };
    let kind = match (float, container_bits) {
        (true, 32) => SampleKind::F32,
        (false, 16) => SampleKind::I16,
        (false, 32) => SampleKind::I32,
        _ => return None,
    };
    Some((Format { kind, channels }, base.nSamplesPerSec))
}

/// Reads sound as Windows hands it over, until stopped (`Ok`) or the
/// default speakers change or fail (`Err`).
fn listen(
    shared: &Shared,
    reader: &IAudioCaptureClient,
    event: &Event,
    (notify, message): (usize, u32),
    format: Format,
    enumerator: &IMMDeviceEnumerator,
    id: &str,
) -> Result<(), String> {
    let mut last_sound: Option<Instant> = None;
    let mut last_check = Instant::now();
    loop {
        if shared.stop.load(Ordering::Acquire) {
            return Ok(());
        }
        // SAFETY: a valid event handle.
        let woke = unsafe { WaitForSingleObject(event.0, WAIT_MS) };
        if woke == WAIT_OBJECT_0 {
            let peak = drain(shared, reader, format)?;
            if peak > SILENCE {
                last_sound = Some(Instant::now());
                if !shared.loud.swap(true, Ordering::AcqRel) {
                    post(notify, message);
                }
            }
        }
        let quiet = last_sound.is_none_or(|t| t.elapsed() > QUIET_AFTER);
        if quiet {
            shared.loud.store(false, Ordering::Release);
        }
        if last_check.elapsed() > DEVICE_CHECK {
            last_check = Instant::now();
            // SAFETY: a valid enumerator.
            let now = unsafe {
                enumerator.GetDefaultAudioEndpoint(eRender, eConsole)
            }
            .ok()
            .map(|d| device_id(&d));
            if now.as_deref() != Some(id) {
                return Err("speakers changed".to_owned());
            }
        }
    }
}

/// Takes every waiting packet into the shared samples. Returns the
/// loudest sample.
fn drain(
    shared: &Shared,
    reader: &IAudioCaptureClient,
    format: Format,
) -> Result<f32, String> {
    let mut peak = 0.0_f32;
    loop {
        // SAFETY: a valid capture client.
        let waiting =
            unsafe { reader.GetNextPacketSize() }.map_err(|e| e.message())?;
        if waiting == 0 {
            return Ok(peak);
        }
        let mut data = std::ptr::null_mut();
        let mut frames = 0_u32;
        let mut flags = 0_u32;
        // SAFETY: valid places to write; the buffer is released below.
        unsafe {
            reader.GetBuffer(
                &raw mut data,
                &raw mut frames,
                &raw mut flags,
                None,
                None,
            )
        }
        .map_err(|e| e.message())?;
        let count = usize::try_from(frames).unwrap_or(0);
        let silent = flags
            & u32::try_from(AUDCLNT_BUFFERFLAGS_SILENT.0).unwrap_or(0)
            != 0;
        if let Ok(mut recent) = shared.recent.lock() {
            if silent || data.is_null() {
                recent.push_silence(count);
            } else {
                let len = count.saturating_mul(format.frame_bytes());
                // SAFETY: Windows gave `frames` frames of `format` at
                // `data`, valid until ReleaseBuffer.
                let bytes = unsafe { std::slice::from_raw_parts(data, len) };
                peak = peak.max(recent.push_frames(bytes, format));
            }
        }
        // SAFETY: releases what GetBuffer handed over.
        unsafe { reader.ReleaseBuffer(frames) }.map_err(|e| e.message())?;
    }
}

/// A device's id, to notice when the default speakers change.
fn device_id(device: &IMMDevice) -> String {
    // SAFETY: a valid device; the string is freed below.
    let Ok(id) = (unsafe { device.GetId() }) else {
        return String::new();
    };
    // SAFETY: a valid wide string from GetId.
    let text = unsafe { id.to_string() }.unwrap_or_default();
    // SAFETY: allocated by GetId with CoTaskMemAlloc.
    unsafe { CoTaskMemFree(Some(id.0.cast_const().cast())) };
    text
}

/// Posts `message` to the window at `notify`.
fn post(notify: usize, message: u32) {
    let hwnd = HWND(std::ptr::with_exposed_provenance_mut(notify));
    // SAFETY: posting is safe from any thread; plain values. If it
    // fails, the bars start with the next sound.
    let _ = unsafe { PostMessageW(Some(hwnd), message, WPARAM(0), LPARAM(0)) };
}
