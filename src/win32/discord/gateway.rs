//! The bot's connection to Discord's gateway: a websocket read on its own
//! thread, heartbeats and the cache updates on another. Discord needs a
//! heartbeat about every 40 seconds; otherwise the bot thread sleeps
//! until a message arrives.

use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::Value;

use super::super::http::{Received, WebSocket};
use crate::discord::bot::{
    Bot, BotStatus, BotView, CLOSE_AUTH_FAILED, CLOSE_DISALLOWED_INTENTS, Reply,
};
use crate::discord::rpc::deadline;
use crate::log;

/// Discord's gateway.
const GATEWAY_HOST: &str = "gateway.discord.gg";
/// Gateway path: API version 10, JSON, no compression.
const GATEWAY_PATH: &str = "/?v=10&encoding=json";
/// First wait before reconnecting.
const RETRY_MIN: Duration = Duration::from_secs(5);
/// Longest wait before reconnecting.
const RETRY_MAX: Duration = Duration::from_secs(60);
/// Changes are handed to the UI at most this often (voice changes go
/// at once, so camera badges aren't late).
const PUBLISH_EVERY: Duration = Duration::from_secs(2);
/// Wait used before the first heartbeat interval is known.
const IDLE_WAIT: Duration = Duration::from_secs(60);
/// Stack size for the gateway threads.
const THREAD_STACK: usize = 512 * 1024;

/// What the bot thread waits for.
pub enum BotInput {
    /// A message from connection number `generation`.
    Message {
        /// Which connection.
        generation: u64,
        /// The text.
        text: String,
    },
    /// Connection `generation` closed with this code (0 if it dropped).
    Closed {
        /// Which connection.
        generation: u64,
        /// The close code.
        code: u16,
    },
    /// Shut down.
    Stop,
}

/// Where the bot hands its view to the UI.
pub struct Publisher {
    /// Shared with the UI.
    pub view: Arc<Mutex<Option<BotView>>>,
    /// Tells the UI there's something new.
    pub notify: Box<dyn Fn() + Send>,
}

impl Publisher {
    /// Stores `view` and tells the UI.
    fn publish(&self, view: BotView) {
        if let Ok(mut shared) = self.view.lock() {
            *shared = Some(view);
        }
        (self.notify)();
    }
}

/// How one connection ended.
enum Outcome {
    /// Stop was asked for.
    Stop,
    /// Connect again after a wait.
    Retry,
    /// Don't retry: the token or intents are wrong.
    Fatal(String),
}

/// The bot thread: connect, run, reconnect with a growing wait.
pub fn run(
    mut bot: Bot,
    input: &Receiver<BotInput>,
    sender: &Sender<BotInput>,
    publisher: &Publisher,
) {
    let mut generation = 0_u64;
    let mut delay = RETRY_MIN;
    loop {
        generation = generation.wrapping_add(1);
        bot.reset();
        publisher.publish(bot.view());
        let started = Instant::now();
        let outcome = match WebSocket::connect(GATEWAY_HOST, GATEWAY_PATH) {
            Ok(socket) => {
                let socket = Arc::new(socket);
                spawn_reader(Arc::clone(&socket), sender.clone(), generation);
                let outcome =
                    serve(&socket, &mut bot, input, generation, publisher);
                socket.close();
                outcome
            }
            Err(error) => {
                log!("Discord bot: {error}");
                Outcome::Retry
            }
        };
        match outcome {
            Outcome::Stop => return,
            Outcome::Fatal(reason) => {
                log!("Discord bot: {reason}");
                publisher.publish(BotView {
                    status: BotStatus::Failed(reason),
                    ..BotView::default()
                });
                // Wait for Stop; retrying won't help.
                while !matches!(input.recv(), Ok(BotInput::Stop) | Err(_)) {}
                return;
            }
            Outcome::Retry => {}
        }
        delay = if started.elapsed() > RETRY_MAX {
            RETRY_MIN
        } else {
            delay.saturating_mul(2).min(RETRY_MAX)
        };
        if !wait(input, delay) {
            return;
        }
    }
}

/// Waits `delay`; returns false if Stop arrived.
fn wait(input: &Receiver<BotInput>, delay: Duration) -> bool {
    let until = deadline(delay);
    loop {
        let remaining = until.saturating_duration_since(Instant::now());
        match input.recv_timeout(remaining) {
            Ok(BotInput::Stop) | Err(RecvTimeoutError::Disconnected) => {
                return false;
            }
            Ok(_) => {}
            Err(RecvTimeoutError::Timeout) => return true,
        }
    }
}

/// Starts the thread that reads messages until the socket closes.
fn spawn_reader(
    socket: Arc<WebSocket>,
    sender: Sender<BotInput>,
    generation: u64,
) {
    let spawned = thread::Builder::new()
        .name("discord-gateway".into())
        .stack_size(THREAD_STACK)
        .spawn(move || {
            loop {
                let input = match socket.receive() {
                    Ok(Received::Text(text)) => {
                        BotInput::Message { generation, text }
                    }
                    Ok(Received::Closed(code)) => {
                        BotInput::Closed { generation, code }
                    }
                    Err(_) => BotInput::Closed {
                        generation,
                        code: 0,
                    },
                };
                let closed = matches!(input, BotInput::Closed { .. });
                if sender.send(input).is_err() || closed {
                    return;
                }
            }
        });
    if let Err(error) = spawned {
        log!("Discord bot: can't start the gateway reader: {error}");
    }
}

/// The heartbeat schedule.
struct Heartbeat {
    /// Interval Discord asked for.
    every: Duration,
    /// When the next one is due.
    next: Instant,
    /// Sent one and no ack yet.
    awaiting_ack: bool,
}

/// Runs one connection until it ends.
fn serve(
    socket: &WebSocket,
    bot: &mut Bot,
    input: &Receiver<BotInput>,
    generation: u64,
    publisher: &Publisher,
) -> Outcome {
    let mut heartbeat: Option<Heartbeat> = None;
    let mut dirty = false;
    let mut last_publish = Instant::now();
    loop {
        let wake = heartbeat
            .as_ref()
            .map_or_else(|| deadline(IDLE_WAIT), |h| h.next);
        let timeout = wake.saturating_duration_since(Instant::now());
        let timeout = if dirty {
            timeout.min(PUBLISH_EVERY)
        } else {
            timeout
        };
        match input.recv_timeout(timeout) {
            Ok(BotInput::Stop) | Err(RecvTimeoutError::Disconnected) => {
                return Outcome::Stop;
            }
            Ok(BotInput::Closed {
                generation: g,
                code,
            }) if g == generation => {
                return closed(code);
            }
            Ok(BotInput::Message {
                generation: g,
                text,
            }) if g == generation => {
                let Ok(message) = serde_json::from_str::<Value>(&text) else {
                    continue;
                };
                if message.get("op").and_then(Value::as_u64)
                    == Some(HEARTBEAT_ACK)
                    && let Some(beat) = heartbeat.as_mut()
                {
                    beat.awaiting_ack = false;
                }
                let (replies, changed) = bot.handle(&message);
                for reply in replies {
                    match reply {
                        Reply::Hello(ms) => {
                            let every = Duration::from_millis(ms);
                            heartbeat = Some(Heartbeat {
                                every,
                                next: deadline(
                                    every.checked_div(2).unwrap_or(every),
                                ),
                                awaiting_ack: false,
                            });
                            if let Err(error) =
                                socket.send_text(&bot.identify().to_string())
                            {
                                log!("Discord bot: {error}");
                                return Outcome::Retry;
                            }
                        }
                        Reply::Send(payload) => {
                            if socket.send_text(&payload.to_string()).is_err() {
                                return Outcome::Retry;
                            }
                        }
                        Reply::Reconnect => return Outcome::Retry,
                    }
                }
                let urgent = message.get("t").and_then(Value::as_str)
                    == Some("VOICE_STATE_UPDATE");
                dirty = dirty || changed;
                if dirty && (urgent || last_publish.elapsed() >= PUBLISH_EVERY)
                {
                    publisher.publish(bot.view());
                    last_publish = Instant::now();
                    dirty = false;
                }
            }
            // From an old connection.
            Ok(BotInput::Message { .. } | BotInput::Closed { .. }) => {}
            Err(RecvTimeoutError::Timeout) => {
                if dirty && last_publish.elapsed() >= PUBLISH_EVERY {
                    publisher.publish(bot.view());
                    last_publish = Instant::now();
                    dirty = false;
                }
                if let Some(beat) = heartbeat.as_mut()
                    && Instant::now() >= beat.next
                {
                    if beat.awaiting_ack {
                        log!("Discord bot: no heartbeat ack; reconnecting");
                        return Outcome::Retry;
                    }
                    if socket.send_text(&bot.heartbeat().to_string()).is_err() {
                        return Outcome::Retry;
                    }
                    beat.awaiting_ack = true;
                    beat.next = deadline(beat.every);
                }
            }
        }
    }
}

/// Gateway opcode for a heartbeat ack.
const HEARTBEAT_ACK: u64 = 11;

/// What a close code means.
fn closed(code: u16) -> Outcome {
    match code {
        CLOSE_AUTH_FAILED => {
            Outcome::Fatal("the bot token was rejected".into())
        }
        CLOSE_DISALLOWED_INTENTS => Outcome::Fatal(
            "turn on the Presence and Server Members intents for the bot"
                .into(),
        ),
        other => {
            log!("Discord bot: gateway closed ({other}); reconnecting");
            Outcome::Retry
        }
    }
}
