//! Runs the Discord session on a background thread and hands its view
//! to the UI thread. A second thread blocks reading the pipe; both feed
//! one channel, so the session thread sleeps until Discord or the UI has
//! something for it (no polling).

mod focus;
mod gateway;
mod pipe;

use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::Value;
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::System::SystemInformation::GetLocalTime;
use windows::Win32::UI::Shell::ShellExecuteW;
use windows::Win32::UI::WindowsAndMessaging::{PostMessageW, SW_SHOWNORMAL};
use windows::core::{HSTRING, PCWSTR, w};

use super::http;
use crate::discord::bot::{Bot, BotView};
use crate::discord::model::DiscordView;
use crate::discord::rpc::{
    Frame, HEADER_BYTES, Incoming, MAX_PAYLOAD, RpcError, Transport, deadline,
    header, parse_header,
};
use crate::discord::session::{
    Command, Credentials, End, Platform, Session, Token,
};
use crate::log;
use gateway::{BotInput, Publisher};
use pipe::Pipe;

/// First wait before reconnecting.
const RETRY_MIN: Duration = Duration::from_secs(2);
/// Longest wait before reconnecting.
const RETRY_MAX: Duration = Duration::from_secs(30);
/// A session this long resets the reconnect wait.
const STABLE_SESSION: Duration = Duration::from_secs(60);
/// Let Discord take a link before raising its window.
const FOCUS_DELAY: Duration = Duration::from_millis(300);
/// Stack size for the background threads (they need very little).
const THREAD_STACK: usize = 256 * 1024;
/// Discord's OAuth token endpoint host and path.
const TOKEN_HOST: &str = "discord.com";
/// Path of the token endpoint.
const TOKEN_PATH: &str = "/api/oauth2/token";
/// Successful HTTP statuses.
const HTTP_OK: std::ops::Range<u32> = 200..300;

/// What the session thread waits for.
enum Input {
    /// A frame from connection number `generation`.
    Frame {
        /// Which connection it came from (stale ones are ignored).
        generation: u64,
        /// The frame.
        frame: Frame,
    },
    /// Connection `generation` closed.
    Closed {
        /// Which connection.
        generation: u64,
        /// Why.
        why: String,
    },
    /// A command from the UI.
    Command(Command),
}

/// The UI's handle on the Discord session. Dropping it stops the
/// session (once any call in progress finishes).
pub struct DiscordLink {
    /// Sends commands to the session thread.
    input: Sender<Input>,
    /// The latest view.
    view: Arc<Mutex<DiscordView>>,
    /// Stops the bot thread, if there is one.
    bot_input: Option<Sender<BotInput>>,
    /// The bot's latest view.
    bot_view: Arc<Mutex<Option<BotView>>>,
}

/// The bot's token and the friends it follows.
pub struct BotSettings {
    /// Bot token.
    pub token: String,
    /// Friends' user ids.
    pub friends: Vec<String>,
}

impl DiscordLink {
    /// Starts the session thread, and the bot thread if `bot` is given.
    /// Each new view is announced by posting `message` to `notify`; the
    /// sign-in token is kept at `token_path`.
    pub fn start(
        credentials: Credentials,
        bot: Option<BotSettings>,
        token_path: PathBuf,
        notify: HWND,
        message: u32,
    ) -> Self {
        let notify_address = notify.0.expose_provenance();
        let bot_view = Arc::new(Mutex::new(None));
        let bot_input = bot.and_then(|settings| {
            start_bot(settings, Arc::clone(&bot_view), notify_address, message)
        });
        let (input, receiver) = mpsc::channel();
        let view = Arc::new(Mutex::new(DiscordView::default()));
        let platform = WinPlatform {
            token_path,
            view: Arc::clone(&view),
            notify: notify.0.expose_provenance(),
            message,
        };
        let sender = input.clone();
        let spawned = thread::Builder::new()
            .name("discord".into())
            .stack_size(THREAD_STACK)
            .spawn(move || run(&credentials, platform, &receiver, &sender));
        if let Err(error) = spawned {
            log!("Discord: can't start its thread: {error}");
        }
        Self {
            input,
            view,
            bot_input,
            bot_view,
        }
    }

    /// The bot's latest view; `None` without a bot.
    pub fn bot_view(&self) -> Option<BotView> {
        self.bot_view.lock().ok().and_then(|v| v.clone())
    }

    /// Sends a command to the session.
    pub fn send(&self, command: Command) {
        // If the thread has gone, there's nobody to tell.
        let _ = self.input.send(Input::Command(command));
    }

    /// The latest view.
    pub fn view(&self) -> DiscordView {
        self.view.lock().map(|v| v.clone()).unwrap_or_default()
    }
}

impl Drop for DiscordLink {
    fn drop(&mut self) {
        self.send(Command::Stop);
        if let Some(bot) = &self.bot_input {
            // If the thread has gone, it's already stopped.
            let _ = bot.send(BotInput::Stop);
        }
    }
}

/// Starts the bot thread. Returns its stop channel.
fn start_bot(
    settings: BotSettings,
    view: Arc<Mutex<Option<BotView>>>,
    notify: usize,
    message: u32,
) -> Option<Sender<BotInput>> {
    let (sender, receiver) = mpsc::channel();
    let thread_sender = sender.clone();
    let publisher = Publisher {
        view,
        notify: Box::new(move || post(notify, message)),
    };
    let bot = Bot::new(settings.token, settings.friends);
    let spawned = thread::Builder::new()
        .name("discord-bot".into())
        .stack_size(THREAD_STACK)
        .spawn(move || {
            gateway::run(bot, &receiver, &thread_sender, &publisher);
        });
    match spawned {
        Ok(_) => Some(sender),
        Err(error) => {
            log!("Discord: can't start the bot thread: {error}");
            None
        }
    }
}

/// Posts `message` to the window at `address`.
fn post(address: usize, message: u32) {
    let hwnd = HWND(std::ptr::with_exposed_provenance_mut(address));
    // SAFETY: posting is safe from any thread. If the window is gone
    // (Wallflex is closing), nobody needs telling.
    let _ = unsafe { PostMessageW(Some(hwnd), message, WPARAM(0), LPARAM(0)) };
}

/// The session thread: connect, run, reconnect with a growing wait.
fn run(
    credentials: &Credentials,
    mut platform: WinPlatform,
    input: &Receiver<Input>,
    sender: &Sender<Input>,
) {
    let mut generation = 0_u64;
    let mut delay = RETRY_MIN;
    let mut last_error = String::new();
    let mut wanted_server = None;
    loop {
        generation = generation.wrapping_add(1);
        let started = Instant::now();
        let end = match Pipe::connect() {
            Ok(pipe) => {
                let pipe = Arc::new(pipe);
                spawn_reader(Arc::clone(&pipe), sender.clone(), generation);
                let transport = PipeTransport {
                    pipe,
                    input,
                    generation,
                };
                Session::new(transport, &mut platform, credentials)
                    .run(wanted_server.clone())
            }
            Err(why) => End::Failed(RpcError::Closed(why)),
        };
        platform.publish(&DiscordView::default());
        match end {
            End::Stopped => return,
            End::Failed(error) => {
                let message = error.to_string();
                if message != last_error {
                    log!("Discord: {message}");
                    last_error = message;
                }
            }
        }
        delay = if started.elapsed() > STABLE_SESSION {
            RETRY_MIN
        } else {
            delay.saturating_mul(2).min(RETRY_MAX)
        };
        if !wait_offline(input, delay, &mut wanted_server) {
            return;
        }
    }
}

/// Waits `delay` before reconnecting, still taking commands. Returns
/// false if the session should stop.
fn wait_offline(
    input: &Receiver<Input>,
    delay: Duration,
    wanted_server: &mut Option<String>,
) -> bool {
    let deadline = deadline(delay);
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        match input.recv_timeout(remaining) {
            Ok(Input::Command(Command::Stop))
            | Err(RecvTimeoutError::Disconnected) => return false,
            Ok(Input::Command(Command::WantServer(server))) => {
                *wanted_server = server;
            }
            Ok(_) => {}
            Err(RecvTimeoutError::Timeout) => return true,
        }
    }
}

/// Starts the thread that reads frames from `pipe` until it closes.
fn spawn_reader(pipe: Arc<Pipe>, sender: Sender<Input>, generation: u64) {
    let spawned = thread::Builder::new()
        .name("discord-pipe".into())
        .stack_size(THREAD_STACK)
        .spawn(move || {
            let why = loop {
                match read_frame(&pipe) {
                    Ok(frame) => {
                        let input = Input::Frame { generation, frame };
                        if sender.send(input).is_err() {
                            return;
                        }
                    }
                    Err(why) => break why,
                }
            };
            let _ = sender.send(Input::Closed { generation, why });
        });
    if let Err(error) = spawned {
        log!("Discord: can't start the pipe reader: {error}");
    }
}

/// Reads one frame.
fn read_frame(pipe: &Pipe) -> Result<Frame, String> {
    let mut bytes = [0_u8; HEADER_BYTES];
    pipe.read_exact(&mut bytes)?;
    let (op, length) = parse_header(bytes);
    if length > MAX_PAYLOAD {
        return Err(format!("frame of {length} bytes is too large"));
    }
    let mut payload =
        vec![0_u8; usize::try_from(length).map_err(|e| e.to_string())?];
    pipe.read_exact(&mut payload)?;
    Ok(Frame { op, payload })
}

/// The pipe plus the shared input channel, as an RPC transport.
struct PipeTransport<'a> {
    /// The connection.
    pipe: Arc<Pipe>,
    /// Frames from the reader thread and commands from the UI.
    input: &'a Receiver<Input>,
    /// This connection's number.
    generation: u64,
}

impl Transport for PipeTransport<'_> {
    type Command = Command;

    fn send(&mut self, frame: &Frame) -> Result<(), RpcError> {
        let length = u32::try_from(frame.payload.len())
            .map_err(|e| RpcError::Protocol(e.to_string()))?;
        let mut bytes = header(frame.op, length).to_vec();
        bytes.extend_from_slice(&frame.payload);
        self.pipe.write_all(&bytes).map_err(RpcError::Closed)
    }

    fn recv(
        &mut self,
        timeout: Duration,
    ) -> Result<Incoming<Command>, RpcError> {
        let deadline = deadline(timeout);
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            match self.input.recv_timeout(remaining) {
                Ok(Input::Frame { generation, frame })
                    if generation == self.generation =>
                {
                    return Ok(Incoming::Frame(frame));
                }
                Ok(Input::Closed { generation, why })
                    if generation == self.generation =>
                {
                    return Err(RpcError::Closed(why));
                }
                Ok(Input::Command(command)) => {
                    return Ok(Incoming::Command(command));
                }
                // From an old connection: ignore.
                Ok(Input::Frame { .. } | Input::Closed { .. }) => {}
                Err(RecvTimeoutError::Timeout) => return Ok(Incoming::Timeout),
                Err(RecvTimeoutError::Disconnected) => {
                    return Err(RpcError::Closed(
                        "Wallflex is stopping".into(),
                    ));
                }
            }
        }
    }
}

impl Drop for PipeTransport<'_> {
    fn drop(&mut self) {
        // Unblocks the reader thread, which then lets the pipe close.
        self.pipe.cancel();
    }
}

/// The session's view of the outside world.
struct WinPlatform {
    /// Where the OAuth token is kept.
    token_path: PathBuf,
    /// Shared with the UI.
    view: Arc<Mutex<DiscordView>>,
    /// The UI window to notify, as an address.
    notify: usize,
    /// The message to post it.
    message: u32,
}

impl Platform for WinPlatform {
    fn token_request(
        &mut self,
        fields: &[(&str, &str)],
    ) -> Result<Value, String> {
        let body = http::form_encode(fields);
        let headers = "Content-Type: application/x-www-form-urlencoded\r\n";
        let response = http::request(
            "POST",
            TOKEN_HOST,
            TOKEN_PATH,
            headers,
            body.as_bytes(),
        )?;
        if !HTTP_OK.contains(&response.status) {
            let text = String::from_utf8_lossy(&response.body);
            return Err(format!("Discord said {}: {text}", response.status));
        }
        serde_json::from_slice(&response.body).map_err(|e| e.to_string())
    }

    fn load_token(&mut self) -> Option<Token> {
        let text = std::fs::read(&self.token_path).ok()?;
        serde_json::from_slice(&text).ok()
    }

    fn save_token(&mut self, token: &Token) {
        let saved = serde_json::to_vec(token)
            .map_err(|e| e.to_string())
            .and_then(|bytes| {
                std::fs::write(&self.token_path, bytes)
                    .map_err(|e| e.to_string())
            });
        if let Err(error) = saved {
            log!("Discord: can't save the token: {error}");
        }
    }

    fn clear_token(&mut self) {
        // Already gone is fine.
        let _ = std::fs::remove_file(&self.token_path);
    }

    fn unix_time(&self) -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_secs())
    }

    fn local_clock(&self) -> String {
        // SAFETY: takes no arguments and returns a plain struct.
        let now = unsafe { GetLocalTime() };
        format!("{:02}:{:02}", now.wHour, now.wMinute)
    }

    fn publish(&mut self, view: &DiscordView) {
        if let Ok(mut shared) = self.view.lock() {
            shared.clone_from(view);
        }
        let hwnd = HWND(std::ptr::with_exposed_provenance_mut(self.notify));
        // SAFETY: posting is safe from any thread. If the window is
        // gone (Wallflex is closing), nobody needs telling.
        let _ = unsafe {
            PostMessageW(Some(hwnd), self.message, WPARAM(0), LPARAM(0))
        };
    }

    fn open(&mut self, link: &str) {
        let link = HSTRING::from(link);
        // SAFETY: the strings are null-terminated and outlive the call.
        // The result is a pseudo-handle that only signals success.
        let result = unsafe {
            ShellExecuteW(
                None,
                w!("open"),
                &link,
                PCWSTR::null(),
                PCWSTR::null(),
                SW_SHOWNORMAL,
            )
        };
        log!("Discord: opening {link} ({:?})", result.0);
        thread::sleep(FOCUS_DELAY);
        if !focus::focus_discord() {
            log!("Discord: its window wasn't found to bring to the front");
        }
    }
}
