//! The Discord client's local RPC protocol, over any [`Transport`].
//!
//! Each message is an 8-byte header (opcode, payload length; both
//! little-endian `u32`) followed by a UTF-8 JSON payload. Ported from the
//! user's `discord_rpc.py`.

use std::collections::VecDeque;
use std::fmt;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

/// Opcode: handshake.
pub const OP_HANDSHAKE: u32 = 0;
/// Opcode: a JSON frame.
pub const OP_FRAME: u32 = 1;
/// Opcode: Discord is closing the connection.
pub const OP_CLOSE: u32 = 2;
/// Opcode: ping.
pub const OP_PING: u32 = 3;
/// Opcode: pong.
pub const OP_PONG: u32 = 4;
/// Header size in bytes.
pub const HEADER_BYTES: usize = 8;
/// Longest payload accepted, so a corrupt length can't exhaust memory.
pub const MAX_PAYLOAD: u32 = 16 * 1024 * 1024;

/// One framed message.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Frame {
    /// Opcode.
    pub op: u32,
    /// JSON payload.
    pub payload: Vec<u8>,
}

/// What a transport delivers while waiting.
#[derive(Debug, PartialEq, Eq)]
pub enum Incoming<C> {
    /// A frame from Discord.
    Frame(Frame),
    /// A command from the UI.
    Command(C),
    /// The wait timed out.
    Timeout,
}

/// Moves frames to and from the Discord client, and delivers commands
/// from the UI on the same wait, so one thread can block on both.
pub trait Transport {
    /// Commands the UI can send.
    type Command;

    /// Sends one frame.
    ///
    /// # Errors
    ///
    /// [`RpcError::Closed`] if the connection is gone.
    fn send(&mut self, frame: &Frame) -> Result<(), RpcError>;

    /// Waits up to `timeout` for the next frame or wake-up.
    ///
    /// # Errors
    ///
    /// [`RpcError::Closed`] if the connection is gone.
    fn recv(
        &mut self,
        timeout: Duration,
    ) -> Result<Incoming<Self::Command>, RpcError>;
}

/// Why an RPC exchange failed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RpcError {
    /// The connection is gone (Discord closed, restarted, not running).
    Closed(String),
    /// Discord answered with an ERROR event.
    Discord {
        /// Discord's error code.
        code: i64,
        /// Its message.
        message: String,
    },
    /// Something didn't parse or didn't arrive in time.
    Protocol(String),
}

impl fmt::Display for RpcError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Closed(why) => write!(f, "disconnected: {why}"),
            Self::Discord { code, message } => {
                write!(f, "Discord error {code}: {message}")
            }
            Self::Protocol(why) => write!(f, "RPC problem: {why}"),
        }
    }
}

impl std::error::Error for RpcError {}

/// The 8-byte header for a frame.
pub fn header(op: u32, length: u32) -> [u8; HEADER_BYTES] {
    let mut bytes = [0_u8; HEADER_BYTES];
    let (op_part, length_part) = bytes.split_at_mut(HEADER_BYTES / 2);
    op_part.copy_from_slice(&op.to_le_bytes());
    length_part.copy_from_slice(&length.to_le_bytes());
    bytes
}

/// The opcode and payload length from a header.
pub fn parse_header(bytes: [u8; HEADER_BYTES]) -> (u32, u32) {
    let (op, length) = bytes.split_at(HEADER_BYTES / 2);
    let word =
        |part: &[u8]| u32::from_le_bytes(part.try_into().unwrap_or_default());
    (word(op), word(length))
}

/// `timeout` from now; "now" if that would overflow the clock.
pub fn deadline(timeout: Duration) -> Instant {
    Instant::now()
        .checked_add(timeout)
        .unwrap_or_else(Instant::now)
}

/// The next thing from [`RpcClient::next`].
#[derive(Debug, PartialEq)]
pub enum Next<C> {
    /// A subscribed event: name and data.
    Event(String, Value),
    /// A command from the UI.
    Command(C),
    /// Nothing arrived in time.
    Timeout,
}

/// One connection to the Discord client. Not thread-safe; use from the
/// session thread only.
pub struct RpcClient<T: Transport> {
    /// The connection.
    transport: T,
    /// Events that arrived while waiting for a reply, oldest first.
    events: VecDeque<(String, Value)>,
    /// Commands that arrived while waiting for a reply, oldest first.
    commands: VecDeque<T::Command>,
    /// Next nonce number.
    nonce: u64,
}

impl<T: Transport> RpcClient<T> {
    /// Wraps a connected transport.
    pub fn new(transport: T) -> Self {
        Self {
            transport,
            events: VecDeque::new(),
            commands: VecDeque::new(),
            nonce: 0,
        }
    }

    /// Does the handshake and returns the READY event's data.
    ///
    /// # Errors
    ///
    /// An [`RpcError`] if the connection fails or READY doesn't arrive
    /// within `timeout`.
    pub fn handshake(
        &mut self,
        client_id: &str,
        timeout: Duration,
    ) -> Result<Value, RpcError> {
        let hello = json!({"v": 1, "client_id": client_id});
        self.send(OP_HANDSHAKE, &hello)?;
        let deadline = deadline(timeout);
        loop {
            let message = self.recv_message(deadline)?;
            if message.get("evt").and_then(Value::as_str) == Some("READY") {
                return Ok(message.get("data").cloned().unwrap_or(Value::Null));
            }
        }
    }

    /// Sends a command and returns its reply's data, waiting up to
    /// `timeout` (AUTHORIZE waits for the user, so it needs minutes).
    ///
    /// # Errors
    ///
    /// [`RpcError::Discord`] for an ERROR reply, or another
    /// [`RpcError`] if the connection fails or the reply is late.
    pub fn call(
        &mut self,
        cmd: &str,
        args: &Value,
        evt: Option<&str>,
        timeout: Duration,
    ) -> Result<Value, RpcError> {
        self.nonce = self.nonce.wrapping_add(1);
        let nonce = self.nonce.to_string();
        let mut payload = json!({"cmd": cmd, "args": args, "nonce": nonce});
        if let (Some(evt), Some(object)) = (evt, payload.as_object_mut()) {
            object.insert("evt".to_owned(), Value::from(evt));
        }
        self.send(OP_FRAME, &payload)?;
        let deadline = deadline(timeout);
        loop {
            let message = self.recv_message(deadline)?;
            if message.get("nonce").and_then(Value::as_str) == Some(&nonce) {
                let data = message.get("data").cloned().unwrap_or(Value::Null);
                if message.get("evt").and_then(Value::as_str) == Some("ERROR") {
                    return Err(RpcError::Discord {
                        code: data
                            .get("code")
                            .and_then(Value::as_i64)
                            .unwrap_or(0),
                        message: crate::discord::model::owned(&data, "message")
                            .unwrap_or_default(),
                    });
                }
                return Ok(data);
            }
            self.queue_if_event(&message);
        }
    }

    /// Subscribes to an event.
    ///
    /// # Errors
    ///
    /// As [`RpcClient::call`].
    pub fn subscribe(
        &mut self,
        evt: &str,
        args: &Value,
        timeout: Duration,
    ) -> Result<Value, RpcError> {
        self.call("SUBSCRIBE", args, Some(evt), timeout)
    }

    /// Unsubscribes from an event.
    ///
    /// # Errors
    ///
    /// As [`RpcClient::call`].
    pub fn unsubscribe(
        &mut self,
        evt: &str,
        args: &Value,
        timeout: Duration,
    ) -> Result<Value, RpcError> {
        self.call("UNSUBSCRIBE", args, Some(evt), timeout)
    }

    /// The next event, command or timeout, waiting up to `timeout`.
    ///
    /// # Errors
    ///
    /// An [`RpcError`] if the connection fails.
    pub fn next(
        &mut self,
        timeout: Duration,
    ) -> Result<Next<T::Command>, RpcError> {
        if let Some(command) = self.commands.pop_front() {
            return Ok(Next::Command(command));
        }
        if let Some((evt, data)) = self.events.pop_front() {
            return Ok(Next::Event(evt, data));
        }
        let deadline = deadline(timeout);
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            match self.transport.recv(remaining)? {
                Incoming::Timeout => return Ok(Next::Timeout),
                Incoming::Command(command) => {
                    return Ok(Next::Command(command));
                }
                Incoming::Frame(frame) => {
                    if let Some(message) = self.handle_frame(&frame)?
                        && let Some((evt, data)) = event_of(&message)
                    {
                        return Ok(Next::Event(evt, data));
                    }
                }
            }
        }
    }

    /// Sends a JSON payload with `op`.
    fn send(&mut self, op: u32, payload: &Value) -> Result<(), RpcError> {
        let bytes = serde_json::to_vec(payload)
            .map_err(|e| RpcError::Protocol(e.to_string()))?;
        self.transport.send(&Frame { op, payload: bytes })
    }

    /// Waits for the next JSON frame, answering pings on the way.
    fn recv_message(&mut self, deadline: Instant) -> Result<Value, RpcError> {
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(RpcError::Protocol("Discord didn't answer".into()));
            }
            match self.transport.recv(remaining)? {
                Incoming::Timeout => {}
                Incoming::Command(command) => self.commands.push_back(command),
                Incoming::Frame(frame) => {
                    if let Some(message) = self.handle_frame(&frame)? {
                        return Ok(message);
                    }
                }
            }
        }
    }

    /// Decodes a frame: JSON frames come back, pings are answered, a
    /// close ends the connection.
    fn handle_frame(
        &mut self,
        frame: &Frame,
    ) -> Result<Option<Value>, RpcError> {
        let message: Value = serde_json::from_slice(&frame.payload)
            .map_err(|e| RpcError::Protocol(e.to_string()))?;
        match frame.op {
            OP_FRAME => Ok(Some(message)),
            OP_PING => {
                self.send(OP_PONG, &message)?;
                Ok(None)
            }
            OP_CLOSE => Err(RpcError::Closed(format!(
                "Discord closed the connection: {message}"
            ))),
            _ => Ok(None),
        }
    }

    /// Keeps a DISPATCH event for later.
    fn queue_if_event(&mut self, message: &Value) {
        if let Some(event) = event_of(message) {
            self.events.push_back(event);
        }
    }
}

/// The event name and data of a DISPATCH message.
fn event_of(message: &Value) -> Option<(String, Value)> {
    if message.get("cmd").and_then(Value::as_str) != Some("DISPATCH") {
        return None;
    }
    let evt = crate::discord::model::owned(message, "evt")?;
    let data = message.get("data").cloned().unwrap_or(Value::Null);
    Some((evt, data))
}

#[cfg(test)]
pub mod tests {
    use std::collections::VecDeque;
    use std::time::Duration;

    use serde_json::{Value, json};

    use super::{
        Frame, Incoming, Next, OP_FRAME, OP_PING, OP_PONG, RpcClient, RpcError,
        Transport, header, parse_header,
    };

    /// A scripted Discord: replies to each command from a list.
    #[derive(Default)]
    pub struct FakeDiscord {
        /// What arrives next, in order.
        pub incoming: VecDeque<Incoming<String>>,
        /// Everything sent.
        pub sent: Vec<Frame>,
        /// Replies by command: (cmd, data) answered with the same nonce.
        pub replies: VecDeque<(String, Value)>,
    }

    impl FakeDiscord {
        pub fn frame(value: &Value) -> Incoming<String> {
            Incoming::Frame(Frame {
                op: OP_FRAME,
                payload: serde_json::to_vec(value).unwrap(),
            })
        }
    }

    impl Transport for FakeDiscord {
        type Command = String;

        fn send(&mut self, frame: &Frame) -> Result<(), RpcError> {
            self.sent.push(frame.clone());
            let message: Value =
                serde_json::from_slice(&frame.payload).unwrap();
            if let (Some(nonce), Some((cmd, data))) =
                (message.get("nonce"), self.replies.pop_front())
            {
                assert_eq!(
                    message["cmd"],
                    Value::from(cmd.as_str()),
                    "{message}"
                );
                let reply = json!({"cmd": cmd, "nonce": nonce, "data": data});
                self.incoming.push_back(Self::frame(&reply));
            }
            Ok(())
        }

        fn recv(
            &mut self,
            _timeout: Duration,
        ) -> Result<Incoming<String>, RpcError> {
            Ok(self.incoming.pop_front().unwrap_or(Incoming::Timeout))
        }
    }

    #[test]
    fn headers_round_trip() {
        assert_eq!(parse_header(header(3, 1234)), (3, 1234));
        assert_eq!(header(1, 2), [1, 0, 0, 0, 2, 0, 0, 0]);
    }

    #[test]
    fn handshake_waits_for_ready() {
        let mut fake = FakeDiscord::default();
        fake.incoming.push_back(Incoming::Command("open".into()));
        fake.incoming.push_back(FakeDiscord::frame(
            &json!({"cmd": "DISPATCH", "evt": "READY", "data": {"v": 1}}),
        ));
        let mut client = RpcClient::new(fake);
        let ready = client.handshake("id", Duration::from_secs(1)).unwrap();
        assert_eq!(ready, json!({"v": 1}));
        // The command during the handshake isn't lost.
        assert_eq!(
            client.next(Duration::ZERO).unwrap(),
            Next::Command("open".into())
        );
    }

    #[test]
    fn call_queues_events_and_answers_pings() {
        let mut fake = FakeDiscord::default();
        // Before the reply: an event and a ping.
        fake.incoming.push_back(FakeDiscord::frame(&json!({
            "cmd": "DISPATCH", "evt": "SPEAKING_START", "data": {"user_id": "1"}
        })));
        fake.incoming.push_back(Incoming::Frame(Frame {
            op: OP_PING,
            payload: b"{}".to_vec(),
        }));
        fake.replies
            .push_back(("GET_GUILDS".into(), json!({"guilds": []})));
        let mut client = RpcClient::new(fake);
        let data = client
            .call("GET_GUILDS", &json!({}), None, Duration::from_secs(1))
            .unwrap();
        assert_eq!(data, json!({"guilds": []}));
        assert_eq!(
            client.next(Duration::ZERO).unwrap(),
            Next::Event("SPEAKING_START".into(), json!({"user_id": "1"}))
        );
        assert!(client.transport.sent.iter().any(|f| f.op == OP_PONG));
    }

    #[test]
    fn errors_become_rpc_errors() {
        let mut fake = FakeDiscord::default();
        fake.replies.push_back(("AUTHENTICATE".into(), json!({})));
        // Replace the scripted reply with an ERROR for the same nonce.
        let mut client = RpcClient::new(fake);
        client.transport.replies.clear();
        client
            .transport
            .incoming
            .push_back(FakeDiscord::frame(&json!({
                "cmd": "AUTHENTICATE", "evt": "ERROR", "nonce": "1",
                "data": {"code": 4009, "message": "Invalid token"}
            })));
        let error = client
            .call("AUTHENTICATE", &json!({}), None, Duration::from_secs(1))
            .unwrap_err();
        assert_eq!(
            error,
            RpcError::Discord {
                code: 4009,
                message: "Invalid token".into()
            }
        );
    }
}
