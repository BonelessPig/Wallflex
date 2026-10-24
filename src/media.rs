//! What's playing: Windows' media sessions (the ones its volume flyout
//! shows) and which to show when there are several. Playing beats
//! paused; music apps beat browsers (`YouTube` and other sites), which
//! beat everything else.

use crate::image::Image;

/// One media session, as Windows reports it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Session {
    /// The app's id, such as `Spotify.exe` or `MSEdge`.
    pub app: String,
    /// Whether it's playing (not paused or stopped).
    pub playing: bool,
    /// Song or video title.
    pub title: String,
    /// Artist or channel.
    pub artist: String,
}

/// What the visualizer shows under its bars.
pub struct NowPlaying {
    /// Song or video title.
    pub title: String,
    /// Artist or channel.
    pub artist: String,
    /// Whether it's playing (a paused one is shown dimmed).
    pub playing: bool,
    /// The album art or video thumbnail, if the app gives one.
    pub thumbnail: Option<Image>,
}

/// A change for the UI.
pub enum MediaChange {
    /// Nothing is playing any more.
    Nothing,
    /// A new song or video.
    Song(NowPlaying),
    /// The same one was paused (`false`) or resumed (`true`).
    Playing(bool),
}

impl Session {
    /// Whether `other` is the same song from the same app (playing or
    /// not).
    pub fn same_song(&self, other: &Self) -> bool {
        self.app == other.app
            && self.title == other.title
            && self.artist == other.artist
    }
}

/// Words in the ids of music apps.
const MUSIC_APPS: [&str; 15] = [
    "spotify",
    "music",
    "tidal",
    "deezer",
    "amazon",
    "foobar",
    "musicbee",
    "winamp",
    "itunes",
    "soundcloud",
    "pandora",
    "plexamp",
    "aimp",
    "qobuz",
    "audirvana",
];
/// Words in the ids of web browsers (`YouTube`, web players).
const BROWSERS: [&str; 8] = [
    "chrome", "msedge", "firefox", "opera", "brave", "vivaldi", "arc", "zen",
];
/// Score for a session that's playing.
const PLAYING: u32 = 100;
/// Score for a music app.
const MUSIC_APP: u32 = 50;
/// Score for a browser.
const BROWSER: u32 = 30;
/// Score for anything else.
const OTHER: u32 = 10;

/// The session to show, if any has a title.
pub fn pick(sessions: &[Session]) -> Option<&Session> {
    sessions
        .iter()
        .filter(|s| !s.title.trim().is_empty())
        .max_by_key(|s| score(s))
}

/// How much `session` deserves showing.
fn score(session: &Session) -> u32 {
    let app = session.app.to_lowercase();
    let kind = if MUSIC_APPS.iter().any(|word| app.contains(word)) {
        MUSIC_APP
    } else if BROWSERS.iter().any(|word| app.contains(word)) {
        BROWSER
    } else {
        OTHER
    };
    let playing = if session.playing { PLAYING } else { 0 };
    kind.saturating_add(playing)
}

#[cfg(test)]
mod tests {
    use super::{Session, pick};

    fn session(app: &str, playing: bool, title: &str) -> Session {
        Session {
            app: app.into(),
            playing,
            title: title.into(),
            artist: String::new(),
        }
    }

    #[test]
    fn music_beats_browsers_beats_others() {
        let sessions = [
            session("vlc.exe", true, "A film"),
            session("MSEdge", true, "A video - YouTube"),
            session("Spotify.exe", true, "A song"),
        ];
        assert_eq!(pick(&sessions).unwrap().title, "A song");
        assert_eq!(pick(&sessions[..2]).unwrap().title, "A video - YouTube");
    }

    #[test]
    fn playing_beats_paused() {
        let sessions = [
            session("Spotify.exe", false, "Paused song"),
            session("chrome", true, "Playing video"),
        ];
        assert_eq!(pick(&sessions).unwrap().title, "Playing video");
    }

    #[test]
    fn untitled_sessions_are_skipped() {
        assert!(pick(&[session("Spotify.exe", true, "  ")]).is_none());
        let apple = session(
            "Microsoft.ZuneMusic_8wekyb3d8bbwe!Microsoft.ZuneMusic",
            false,
            "Media Player song",
        );
        let game = session("game.exe", true, "Menu music");
        // A playing game beats a paused player.
        assert_eq!(pick(&[apple, game]).unwrap().title, "Menu music");
    }
}
