//! Watches what's playing through Windows' media controls (the sessions
//! the volume flyout shows), on its own thread. Every couple of seconds
//! it picks the session to show (see [`crate::media::pick`]); when the
//! song changes it fetches the thumbnail and tells the UI.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use windows::Media::Control::{
    GlobalSystemMediaTransportControlsSession as MediaSession,
    GlobalSystemMediaTransportControlsSessionManager as SessionManager,
    GlobalSystemMediaTransportControlsSessionPlaybackStatus as PlaybackStatus,
};
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::Graphics::Imaging::{
    CLSID_WICImagingFactory, IWICImagingFactory,
};
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED, CoCreateInstance,
    CoInitializeEx, IStream,
};
use windows::Win32::System::WinRT::CreateStreamOverRandomAccessStream;
use windows::Win32::UI::WindowsAndMessaging::PostMessageW;

use super::images::decode_stream;
use crate::image::Image;
use crate::log;
use crate::media::{MediaChange, NowPlaying, Session, pick};

/// Stack size for the watcher thread.
const THREAD_STACK: usize = 512 * 1024;
/// How often the sessions are checked.
const CHECK_EVERY: Duration = Duration::from_secs(2);
/// Steps of that wait, so a stop is noticed quickly.
const WAIT_STEP: Duration = Duration::from_millis(250);
/// Thumbnails are decoded at this size (pixels), enough for 4K.
const THUMBNAIL_PX: u32 = 256;

/// What the watcher shares with the UI.
#[derive(Default)]
struct Shared {
    /// The latest change, waiting for the UI.
    latest: Mutex<Option<MediaChange>>,
    /// Set to stop the thread.
    stop: AtomicBool,
}

/// The running watcher; stopped on drop.
pub struct MediaWatcher {
    /// Shared with the thread.
    shared: Arc<Shared>,
    /// The thread, joined on drop.
    thread: Option<JoinHandle<()>>,
}

impl MediaWatcher {
    /// Starts watching. Each change is announced by posting `message` to
    /// `notify`; collect it with [`MediaWatcher::take`].
    pub fn start(notify: HWND, message: u32) -> Self {
        let shared = Arc::new(Shared::default());
        let theirs = Arc::clone(&shared);
        let notify = notify.0.expose_provenance();
        let thread = thread::Builder::new()
            .name("media".into())
            .stack_size(THREAD_STACK)
            .spawn(move || run(&theirs, notify, message));
        let thread = match thread {
            Ok(thread) => Some(thread),
            Err(error) => {
                log!("can't start the media thread: {error}");
                None
            }
        };
        Self { shared, thread }
    }

    /// The latest change, if there's one waiting.
    pub fn take(&self) -> Option<MediaChange> {
        self.shared.latest.lock().ok()?.take()
    }
}

impl Drop for MediaWatcher {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::Release);
        if let Some(thread) = self.thread.take() {
            // The thread notices within WAIT_STEP.
            let _ = thread.join();
        }
    }
}

/// The thread: checks the sessions until stopped.
fn run(shared: &Shared, notify: usize, message: u32) {
    // SAFETY: once per thread, before any COM or WinRT call.
    let _ = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
    // SAFETY: creates WIC's factory, an in-process COM object.
    let factory: Option<IWICImagingFactory> = unsafe {
        CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER)
    }
    .ok();
    let manager = SessionManager::RequestAsync().and_then(|op| op.join());
    let manager = match manager {
        Ok(manager) => manager,
        Err(error) => {
            log!("media: can't watch what's playing ({})", error.message());
            return;
        }
    };
    // What was last shown.
    let mut shown: Option<Session> = None;
    while !shared.stop.load(Ordering::Acquire) {
        let sessions = read_sessions(&manager);
        let details: Vec<Session> =
            sessions.iter().map(|(s, _)| s.clone()).collect();
        let best = pick(&details).cloned();
        let change = match (&best, &shown) {
            (None, None) => None,
            (None, Some(_)) => Some(MediaChange::Nothing),
            (Some(b), Some(s)) if b.same_song(s) => (b.playing != s.playing)
                .then_some(MediaChange::Playing(b.playing)),
            (Some(b), _) => {
                let session = sessions.iter().find(|(s, _)| s == b);
                Some(MediaChange::Song(NowPlaying {
                    title: b.title.clone(),
                    artist: b.artist.clone(),
                    playing: b.playing,
                    thumbnail: session.and_then(|(_, session)| {
                        thumbnail(session, factory.as_ref())
                    }),
                }))
            }
        };
        shown = best;
        if let Some(change) = change {
            hand_over(shared, change);
            post(notify, message);
        }
        let mut waited = Duration::ZERO;
        while waited < CHECK_EVERY && !shared.stop.load(Ordering::Acquire) {
            thread::sleep(WAIT_STEP);
            waited = waited.saturating_add(WAIT_STEP);
        }
    }
}

/// Leaves `change` for the UI. A play/pause change after a new song
/// still waiting just updates that song.
fn hand_over(shared: &Shared, change: MediaChange) {
    let Ok(mut latest) = shared.latest.lock() else {
        return;
    };
    *latest = match (latest.take(), change) {
        (Some(MediaChange::Song(mut song)), MediaChange::Playing(playing)) => {
            song.playing = playing;
            Some(MediaChange::Song(song))
        }
        (_, change) => Some(change),
    };
}

/// Every session with its details. Sessions that fail to answer are
/// left out.
fn read_sessions(manager: &SessionManager) -> Vec<(Session, MediaSession)> {
    let Ok(list) = manager.GetSessions() else {
        return Vec::new();
    };
    let count = list.Size().unwrap_or(0);
    (0..count)
        .filter_map(|index| {
            let session = list.GetAt(index).ok()?;
            let app = session.SourceAppUserModelId().ok()?.to_string();
            let playing = session
                .GetPlaybackInfo()
                .and_then(|info| info.PlaybackStatus())
                .is_ok_and(|status| status == PlaybackStatus::Playing);
            let properties =
                session.TryGetMediaPropertiesAsync().ok()?.join().ok()?;
            let title = properties.Title().ok()?.to_string();
            let artist = properties
                .Artist()
                .map(|a| a.to_string())
                .unwrap_or_default();
            Some((
                Session {
                    app,
                    playing,
                    title,
                    artist,
                },
                session,
            ))
        })
        .collect()
}

/// The session's album art or video thumbnail, if it has one.
fn thumbnail(
    session: &MediaSession,
    factory: Option<&IWICImagingFactory>,
) -> Option<Image> {
    let factory = factory?;
    let properties = session.TryGetMediaPropertiesAsync().ok()?.join().ok()?;
    let reference = properties.Thumbnail().ok()?;
    let stream = reference.OpenReadAsync().ok()?.join().ok()?;
    // SAFETY: wraps the WinRT stream in a COM IStream for WIC; plain
    // interface conversion.
    let stream: IStream =
        unsafe { CreateStreamOverRandomAccessStream(&stream) }.ok()?;
    decode_stream(factory, &stream, THUMBNAIL_PX)
        .map_err(|error| log!("media: thumbnail: {error}"))
        .ok()
}

/// Posts `message` to the window at `notify`.
fn post(notify: usize, message: u32) {
    let hwnd = HWND(std::ptr::with_exposed_provenance_mut(notify));
    // SAFETY: posting is safe from any thread; plain values.
    let _ = unsafe { PostMessageW(Some(hwnd), message, WPARAM(0), LPARAM(0)) };
}
