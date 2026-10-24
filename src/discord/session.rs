//! The Discord session: sign in, follow your call and notifications, and
//! read the SERVER page's voice channels. Ported from the user's
//! `DiscordBridge` (`Bridge` in `discord_bridge.py`).

use std::collections::HashMap;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::model::{
    self, DIRECT_CALL, DiscordView, Guild, NOTIFICATION_LIMIT, ServerView,
    Status, Voice, VoiceChannel, discord_link, is_id, member_from,
    notification_from, owned, text,
};
use super::rpc::{Next, RpcClient, RpcError, Transport};
use crate::log;

/// OAuth scopes Wallflex asks for.
const SCOPES: [&str; 4] = [
    "rpc",
    "rpc.voice.read",
    "rpc.notifications.read",
    "identify",
];
/// Discord's codes for a stored token that's no longer usable.
const TOKEN_ERRORS: [i64; 3] = [4000, 4006, 4009];
/// Events subscribed for the whole session.
const GLOBAL_EVENTS: [&str; 2] =
    ["VOICE_CHANNEL_SELECT", "NOTIFICATION_CREATE"];
/// Events subscribed per voice channel.
const VOICE_EVENTS: [&str; 5] = [
    "VOICE_STATE_CREATE",
    "VOICE_STATE_UPDATE",
    "VOICE_STATE_DELETE",
    "SPEAKING_START",
    "SPEAKING_STOP",
];
/// Channel types that are voice channels (voice, stage).
const VOICE_CHANNEL_TYPES: [u64; 2] = [2, 13];
/// Longest wait for an ordinary reply.
const CALL_TIMEOUT: Duration = Duration::from_secs(10);
/// AUTHORIZE waits for the user to click Authorize in Discord.
const AUTHORIZE_TIMEOUT: Duration = Duration::from_secs(300);
/// Longest wait for an event before periodic work.
const TICK: Duration = Duration::from_millis(500);
/// How often the SERVER page's channels are re-read while it's open.
const SERVER_POLL: Duration = Duration::from_secs(3);
/// How often a server's channel list is re-read.
const CHANNEL_LIST: Duration = Duration::from_secs(60);
/// How often your server list is re-read.
const GUILD_LIST: Duration = Duration::from_secs(300);
/// Voice channels read per SERVER refresh.
const MAX_VOICE_CHANNELS: usize = 25;
/// Refresh the token this long before it expires, in seconds.
const TOKEN_REFRESH_MARGIN_S: u64 = 300;

/// Your Discord application's details, from `[discord]` in the config.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Credentials {
    /// Application (client) id.
    pub client_id: String,
    /// Client secret, for the OAuth token exchange.
    pub client_secret: String,
    /// The redirect URI registered in the developer portal.
    pub redirect_uri: String,
}

/// A stored OAuth token.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[expect(
    clippy::struct_field_names,
    reason = "named after OAuth's own fields, which the file stores"
)]
pub struct Token {
    /// Bearer token for AUTHENTICATE.
    pub access_token: String,
    /// For getting a new access token.
    pub refresh_token: String,
    /// Expiry, in seconds since 1970.
    pub expires_at: u64,
}

/// What the UI can ask of the session.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Command {
    /// Open Discord on a notification's channel.
    Open {
        /// The notification's channel.
        channel_id: String,
        /// Its message, to scroll to.
        message_id: Option<String>,
    },
    /// The SERVER page wants this server's channels (`None`: page closed).
    WantServer(Option<String>),
    /// Shut down.
    Stop,
}

/// Things the session needs from the outside world.
pub trait Platform {
    /// POSTs form fields to Discord's OAuth token endpoint and returns
    /// the JSON reply.
    ///
    /// # Errors
    ///
    /// A message when the request fails.
    fn token_request(
        &mut self,
        fields: &[(&str, &str)],
    ) -> Result<Value, String>;
    /// The stored token, if any.
    fn load_token(&mut self) -> Option<Token>;
    /// Stores a token.
    fn save_token(&mut self, token: &Token);
    /// Forgets the stored token.
    fn clear_token(&mut self);
    /// Seconds since 1970.
    fn unix_time(&self) -> u64;
    /// The local time as `HH:MM`.
    fn local_clock(&self) -> String;
    /// Hands a new view to the UI.
    fn publish(&mut self, view: &DiscordView);
    /// Opens a `discord://` link and brings Discord to the front.
    fn open(&mut self, link: &str);
}

/// How a session ended.
#[derive(Debug, PartialEq, Eq)]
pub enum End {
    /// The UI asked it to stop.
    Stopped,
    /// The connection or Discord failed.
    Failed(RpcError),
}

/// One connected session.
pub struct Session<'a, T: Transport<Command = Command>, P: Platform> {
    /// The RPC connection.
    rpc: RpcClient<T>,
    /// The outside world.
    platform: &'a mut P,
    /// The application's details.
    credentials: &'a Credentials,
    /// What the widget shows.
    view: DiscordView,
    /// What was last published.
    published: Option<DiscordView>,
    /// Your user id.
    self_id: Option<String>,
    /// Next join number in the current call.
    join_seq: u64,
    /// Next notification number.
    note_seq: u64,
    /// Server names by id.
    guild_names: HashMap<String, String>,
    /// Each channel's server (`None`: a DM), for notification links.
    channel_guilds: HashMap<String, Option<String>>,
    /// When the server list was last read.
    guilds_at: Option<Instant>,
    /// The server the SERVER page wants.
    wanted_server: Option<String>,
    /// When its channels were last read.
    server_at: Option<Instant>,
    /// The server whose channel list is cached.
    server_guild: Option<String>,
    /// That server's voice channels.
    server_channels: Vec<Value>,
    /// When the channel list was read.
    channels_at: Option<Instant>,
}

impl<'a, T: Transport<Command = Command>, P: Platform> Session<'a, T, P> {
    /// Wraps a connected transport.
    pub fn new(
        transport: T,
        platform: &'a mut P,
        credentials: &'a Credentials,
    ) -> Self {
        Self {
            rpc: RpcClient::new(transport),
            platform,
            credentials,
            view: DiscordView::default(),
            published: None,
            self_id: None,
            join_seq: 0,
            note_seq: 0,
            guild_names: HashMap::new(),
            channel_guilds: HashMap::new(),
            guilds_at: None,
            wanted_server: None,
            server_at: None,
            server_guild: None,
            server_channels: Vec::new(),
            channels_at: None,
        }
    }

    /// Runs until the connection drops or the UI says stop.
    pub fn run(mut self, wanted_server: Option<String>) -> End {
        self.wanted_server = wanted_server;
        match self.serve() {
            Ok(()) => End::Stopped,
            Err(error) => End::Failed(error),
        }
    }

    /// Signs in, subscribes, then handles events until stopped.
    fn serve(&mut self) -> Result<(), RpcError> {
        self.rpc
            .handshake(&self.credentials.client_id, CALL_TIMEOUT)?;
        let user = self.authenticate()?;
        self.self_id = owned(&user, "id");
        self.view.user =
            owned(&user, "global_name").or_else(|| owned(&user, "username"));
        self.view.status = Status::Ready;
        log!(
            "Discord: signed in as {}",
            self.view.user.as_deref().unwrap_or("?")
        );

        for evt in GLOBAL_EVENTS {
            self.rpc.subscribe(evt, &json!({}), CALL_TIMEOUT)?;
        }
        let selected = self.call("GET_SELECTED_VOICE_CHANNEL", &json!({}))?;
        self.select_channel(&selected)?;
        self.publish();

        loop {
            match self.rpc.next(TICK)? {
                Next::Event(evt, data) => self.handle(&evt, &data)?,
                Next::Command(Command::Stop) => return Ok(()),
                Next::Command(command) => self.command(command),
                Next::Timeout => {}
            }
            self.tick()?;
            self.publish();
        }
    }

    /// Sends a command with the usual timeout.
    fn call(&mut self, cmd: &str, args: &Value) -> Result<Value, RpcError> {
        self.rpc.call(cmd, args, None, CALL_TIMEOUT)
    }

    /// Hands the view to the UI if it changed.
    fn publish(&mut self) {
        if self.published.as_ref() != Some(&self.view) {
            self.platform.publish(&self.view);
            self.published = Some(self.view.clone());
        }
    }

    // -- signing in -----------------------------------------------------

    /// AUTHENTICATE with the stored token (refreshed if it's about to
    /// expire), or ask the user to authorize Wallflex. Returns the user.
    fn authenticate(&mut self) -> Result<Value, RpcError> {
        let mut token = self.platform.load_token();
        let now = self.platform.unix_time();
        if let Some(stale) = token.as_ref().filter(|t| {
            now.saturating_add(TOKEN_REFRESH_MARGIN_S) > t.expires_at
        }) {
            let refresh = stale.refresh_token.clone();
            token = match self.exchange(&[
                ("grant_type", "refresh_token"),
                ("refresh_token", &refresh),
            ]) {
                Ok(fresh) => {
                    log!("Discord: refreshed the access token");
                    Some(fresh)
                }
                Err(error) => {
                    log!(
                        "Discord: token refresh failed ({error}); asking again"
                    );
                    None
                }
            };
        }
        if let Some(token) = token {
            let args = json!({"access_token": token.access_token});
            match self.call("AUTHENTICATE", &args) {
                Ok(data) => {
                    return Ok(data
                        .get("user")
                        .cloned()
                        .unwrap_or(Value::Null));
                }
                Err(RpcError::Discord { code, message })
                    if TOKEN_ERRORS.contains(&code) =>
                {
                    log!(
                        "Discord: stored token rejected ({message}); asking again"
                    );
                    self.platform.clear_token();
                }
                Err(error) => return Err(error),
            }
        }
        self.authorize()
    }

    /// Asks the user to approve Wallflex in Discord, then signs in.
    fn authorize(&mut self) -> Result<Value, RpcError> {
        log!("Discord: asking for permission; approve Wallflex in Discord");
        self.view.status = Status::Authorizing;
        self.publish();
        let args =
            json!({"client_id": self.credentials.client_id, "scopes": SCOPES});
        let reply =
            self.rpc.call("AUTHORIZE", &args, None, AUTHORIZE_TIMEOUT)?;
        let code = text(&reply, "code")
            .ok_or_else(|| RpcError::Protocol("AUTHORIZE gave no code".into()))?
            .to_owned();
        let redirect = self.credentials.redirect_uri.clone();
        let token = self
            .exchange(&[
                ("grant_type", "authorization_code"),
                ("code", &code),
                ("redirect_uri", &redirect),
            ])
            .map_err(RpcError::Protocol)?;
        let data = self.call(
            "AUTHENTICATE",
            &json!({"access_token": token.access_token}),
        )?;
        Ok(data.get("user").cloned().unwrap_or(Value::Null))
    }

    /// Runs a token request and stores the result.
    fn exchange(&mut self, fields: &[(&str, &str)]) -> Result<Token, String> {
        let mut all = vec![
            ("client_id", self.credentials.client_id.as_str()),
            ("client_secret", self.credentials.client_secret.as_str()),
        ];
        all.extend_from_slice(fields);
        let reply = self.platform.token_request(&all)?;
        let token = token_from(&reply, self.platform.unix_time())?;
        self.platform.save_token(&token);
        Ok(token)
    }

    // -- events and commands --------------------------------------------

    /// Applies one event.
    fn handle(&mut self, evt: &str, data: &Value) -> Result<(), RpcError> {
        match evt {
            "VOICE_CHANNEL_SELECT" => {
                let channel = match text(data, "channel_id") {
                    Some(id) => {
                        self.call("GET_CHANNEL", &json!({"channel_id": id}))?
                    }
                    None => Value::Null,
                };
                self.select_channel(&channel)?;
            }
            "VOICE_STATE_CREATE" | "VOICE_STATE_UPDATE" => {
                self.upsert_member(data);
            }
            "VOICE_STATE_DELETE" => {
                let user = data.get("user").unwrap_or(&Value::Null);
                if let (Some(voice), Some(id)) =
                    (self.view.voice.as_mut(), text(user, "id"))
                {
                    voice.members.retain(|m| m.id != id);
                }
            }
            "SPEAKING_START" | "SPEAKING_STOP" => {
                let speaking = evt == "SPEAKING_START";
                if let (Some(voice), Some(id)) =
                    (self.view.voice.as_mut(), text(data, "user_id"))
                {
                    for member in
                        voice.members.iter_mut().filter(|m| m.id == id)
                    {
                        member.speaking = speaking;
                    }
                }
            }
            "NOTIFICATION_CREATE" => self.add_notification(data),
            other => log!("Discord: unhandled event {other}"),
        }
        Ok(())
    }

    /// Applies a UI command.
    fn command(&mut self, command: Command) {
        match command {
            Command::Open {
                channel_id,
                message_id,
            } => self.open(&channel_id, message_id.as_deref()),
            Command::WantServer(server) => {
                if server != self.wanted_server {
                    self.wanted_server = server;
                    self.server_at = None;
                    self.view.server = None;
                }
            }
            Command::Stop => {}
        }
    }

    /// Opens a channel from a recent notification. Only those, so a
    /// stray click can't open anything else.
    fn open(&mut self, channel_id: &str, message_id: Option<&str>) {
        let message_id = message_id.filter(|m| is_id(m));
        let note = self
            .view
            .notifications
            .iter()
            .find(|n| n.channel_id.as_deref() == Some(channel_id));
        if let Some(note) = note.filter(|_| is_id(channel_id)) {
            let link =
                discord_link(note.guild_id.as_deref(), channel_id, message_id);
            self.platform.open(&link);
        }
    }

    /// Switches the voice subscriptions to `channel` (null: no call).
    fn select_channel(&mut self, channel: &Value) -> Result<(), RpcError> {
        if let Some(old) = self.view.voice.take() {
            for evt in VOICE_EVENTS {
                let args = json!({"channel_id": old.channel_id});
                if let Err(error) =
                    self.rpc.unsubscribe(evt, &args, CALL_TIMEOUT)
                {
                    log!("Discord: unsubscribe {evt} failed: {error}");
                }
            }
        }
        self.join_seq = 0;
        let Some(channel_id) = owned(channel, "id") else {
            log!("Discord: not in voice");
            return Ok(());
        };
        for evt in VOICE_EVENTS {
            self.rpc.subscribe(
                evt,
                &json!({"channel_id": channel_id}),
                CALL_TIMEOUT,
            )?;
        }
        let guild_id = owned(channel, "guild_id");
        let guild = match &guild_id {
            Some(id) => self.guild_name(id),
            None => DIRECT_CALL.to_owned(),
        };
        self.view.voice = Some(Voice {
            channel: owned(channel, "name")
                .unwrap_or_else(|| "CALL".to_owned()),
            channel_id,
            guild,
            guild_id,
            members: Vec::new(),
        });
        for entry in channel
            .get("voice_states")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            self.upsert_member(entry);
        }
        if let Some(voice) = &self.view.voice {
            log!(
                "Discord: in {} / {} ({} people)",
                voice.guild,
                voice.channel,
                voice.members.len()
            );
        }
        Ok(())
    }

    /// Adds or updates someone in the call, keeping their join order and
    /// speaking state.
    fn upsert_member(&mut self, entry: &Value) {
        let Some(mut member) = member_from(entry, self.self_id.as_deref())
        else {
            return;
        };
        let Some(voice) = self.view.voice.as_mut() else {
            return;
        };
        if let Some(old) = voice.members.iter_mut().find(|m| m.id == member.id)
        {
            member.join = old.join;
            member.speaking = old.speaking;
            *old = member;
        } else {
            self.join_seq = self.join_seq.saturating_add(1);
            member.join = self.join_seq;
            voice.members.push(member);
        }
    }

    /// Keeps a notification, newest first.
    fn add_notification(&mut self, data: &Value) {
        let guild_id = text(data, "channel_id")
            .map(str::to_owned)
            .and_then(|id| self.channel_guild(&id));
        self.note_seq = self.note_seq.saturating_add(1);
        let clock = self.platform.local_clock();
        let note = notification_from(data, guild_id, self.note_seq, clock);
        self.view.notifications.push_front(note);
        self.view.notifications.truncate(NOTIFICATION_LIMIT);
    }

    /// A channel's server (`None` for DMs or if unknown), cached.
    fn channel_guild(&mut self, channel_id: &str) -> Option<String> {
        if let Some(known) = self.channel_guilds.get(channel_id) {
            return known.clone();
        }
        match self.call("GET_CHANNEL", &json!({"channel_id": channel_id})) {
            Ok(info) => {
                let guild = owned(&info, "guild_id");
                self.channel_guilds
                    .insert(channel_id.to_owned(), guild.clone());
                guild
            }
            Err(error) => {
                log!("Discord: GET_CHANNEL {channel_id} failed: {error}");
                None
            }
        }
    }

    /// A server's name, cached.
    fn guild_name(&mut self, guild_id: &str) -> String {
        if let Some(name) = self.guild_names.get(guild_id) {
            return name.clone();
        }
        match self.call("GET_GUILD", &json!({"guild_id": guild_id})) {
            Ok(data) => {
                let name = owned(&data, "name").unwrap_or_default();
                self.guild_names.insert(guild_id.to_owned(), name.clone());
                name
            }
            Err(error) => {
                log!("Discord: GET_GUILD failed: {error}");
                String::new()
            }
        }
    }

    // -- periodic work --------------------------------------------------

    /// Re-reads the server list and the SERVER page's channels when due.
    fn tick(&mut self) -> Result<(), RpcError> {
        if is_due(self.guilds_at, GUILD_LIST) {
            self.guilds_at = Some(Instant::now());
            self.refresh_guilds();
        }
        if let Some(wanted) = self.wanted_server.clone()
            && is_due(self.server_at, SERVER_POLL)
        {
            self.server_at = Some(Instant::now());
            self.poll_server(&wanted)?;
        }
        Ok(())
    }

    /// Reads your server list.
    fn refresh_guilds(&mut self) {
        let data = match self.call("GET_GUILDS", &json!({})) {
            Ok(data) => data,
            Err(error) => {
                log!("Discord: GET_GUILDS failed: {error}");
                return;
            }
        };
        let guilds = data
            .get("guilds")
            .and_then(Value::as_array)
            .into_iter()
            .flatten();
        self.view.servers = guilds
            .filter_map(|g| {
                Some(Guild {
                    id: owned(g, "id")?,
                    name: owned(g, "name").unwrap_or_default(),
                    icon: owned(g, "icon_url"),
                })
            })
            .collect();
        for guild in &self.view.servers {
            self.guild_names
                .insert(guild.id.clone(), guild.name.clone());
        }
    }

    /// Reads who's in each voice channel of `guild_id`.
    fn poll_server(&mut self, guild_id: &str) -> Result<(), RpcError> {
        let channels = match self.voice_channels(guild_id) {
            Ok(channels) => channels,
            Err(error @ RpcError::Discord { .. }) => {
                log!("Discord: reading server {guild_id} failed: {error}");
                self.view.server = None;
                return Ok(());
            }
            Err(error) => return Err(error),
        };
        let mut found = Vec::new();
        for channel in channels.iter().take(MAX_VOICE_CHANNELS) {
            if let Some(id) = text(channel, "id") {
                found.push(
                    self.call("GET_CHANNEL", &json!({"channel_id": id}))?,
                );
            }
        }
        found.sort_by_key(|info| {
            info.get("position").and_then(Value::as_i64).unwrap_or(0)
        });
        let you = self.view.voice.as_ref().map(|v| v.channel_id.clone());
        let occupied: Vec<VoiceChannel> = found
            .iter()
            .filter_map(|info| {
                let states =
                    info.get("voice_states").and_then(Value::as_array)?;
                let members: Vec<_> = states
                    .iter()
                    .filter_map(|e| member_from(e, self.self_id.as_deref()))
                    .collect();
                let id = owned(info, "id")?;
                (!members.is_empty()).then(|| VoiceChannel {
                    you: you.as_deref() == Some(id.as_str()),
                    name: owned(info, "name").unwrap_or_default(),
                    id,
                    members,
                })
            })
            .collect();
        let icon = self
            .view
            .servers
            .iter()
            .find(|g| g.id == guild_id)
            .and_then(|g| g.icon.clone());
        self.view.server = Some(ServerView {
            id: guild_id.to_owned(),
            name: self.guild_name(guild_id),
            icon,
            empty: found.len().saturating_sub(occupied.len()),
            channels: occupied,
        });
        Ok(())
    }

    /// The server's voice channels, re-read every [`CHANNEL_LIST`].
    fn voice_channels(
        &mut self,
        guild_id: &str,
    ) -> Result<Vec<Value>, RpcError> {
        let cached = self.server_guild.as_deref() == Some(guild_id);
        if !cached || is_due(self.channels_at, CHANNEL_LIST) {
            let data =
                self.call("GET_CHANNELS", &json!({"guild_id": guild_id}))?;
            self.server_channels = data
                .get("channels")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter(|c| {
                    c.get("type")
                        .and_then(Value::as_u64)
                        .is_some_and(|t| VOICE_CHANNEL_TYPES.contains(&t))
                })
                .cloned()
                .collect();
            self.server_guild = Some(guild_id.to_owned());
            self.channels_at = Some(Instant::now());
        }
        Ok(self.server_channels.clone())
    }
}

/// Whether something last done at `last` is due again after `every`.
fn is_due(last: Option<Instant>, every: Duration) -> bool {
    last.is_none_or(|t| t.elapsed() >= every)
}

/// A token from Discord's token endpoint reply.
fn token_from(reply: &Value, now: u64) -> Result<Token, String> {
    let access_token =
        model::owned(reply, "access_token").ok_or_else(|| {
            format!("no access token in Discord's reply: {reply}")
        })?;
    let expires_in =
        reply.get("expires_in").and_then(Value::as_u64).unwrap_or(0);
    Ok(Token {
        access_token,
        refresh_token: model::owned(reply, "refresh_token").unwrap_or_default(),
        expires_at: now.saturating_add(expires_in),
    })
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use serde_json::{Value, json};

    use super::{Command, Credentials, End, Platform, Session, Token};
    use crate::discord::model::{DiscordView, Status};
    use crate::discord::rpc::{Frame, Incoming, RpcError, Transport};
    use std::collections::VecDeque;

    /// A scripted Discord that answers commands from a table.
    struct Fake {
        incoming: VecDeque<Incoming<Command>>,
        answers: Vec<(String, Value)>,
        sent: Vec<Value>,
    }

    impl Fake {
        fn new(answers: &[(&str, Value)]) -> Self {
            let ready = json!({"cmd": "DISPATCH", "evt": "READY", "data": {}});
            Self {
                incoming: VecDeque::from([frame(&ready)]),
                answers: answers
                    .iter()
                    .map(|(c, v)| ((*c).to_owned(), v.clone()))
                    .collect(),
                sent: Vec::new(),
            }
        }
    }

    fn frame(value: &Value) -> Incoming<Command> {
        Incoming::Frame(Frame {
            op: 1,
            payload: serde_json::to_vec(value).unwrap(),
        })
    }

    impl Transport for Fake {
        type Command = Command;

        fn send(&mut self, frame: &Frame) -> Result<(), RpcError> {
            let message: Value =
                serde_json::from_slice(&frame.payload).unwrap();
            if let Some(nonce) = message.get("nonce") {
                let cmd = message["cmd"].as_str().unwrap().to_owned();
                let data = self
                    .answers
                    .iter()
                    .find(|(c, _)| *c == cmd)
                    .map_or(json!({}), |(_, v)| v.clone());
                self.incoming.push_front(frame_reply(&cmd, nonce, &data));
            }
            self.sent.push(message);
            Ok(())
        }

        fn recv(&mut self, _: Duration) -> Result<Incoming<Command>, RpcError> {
            self.incoming
                .pop_front()
                .ok_or_else(|| RpcError::Closed("script over".into()))
        }
    }

    fn frame_reply(
        cmd: &str,
        nonce: &Value,
        data: &Value,
    ) -> Incoming<Command> {
        frame(&json!({"cmd": cmd, "nonce": nonce, "data": data}))
    }

    #[derive(Default)]
    struct World {
        token: Option<Token>,
        views: Vec<DiscordView>,
        opened: Vec<String>,
    }

    impl Platform for World {
        fn token_request(
            &mut self,
            _: &[(&str, &str)],
        ) -> Result<Value, String> {
            Ok(
                json!({"access_token": "new", "refresh_token": "r", "expires_in": 600}),
            )
        }
        fn load_token(&mut self) -> Option<Token> {
            self.token.clone()
        }
        fn save_token(&mut self, token: &Token) {
            self.token = Some(token.clone());
        }
        fn clear_token(&mut self) {
            self.token = None;
        }
        fn unix_time(&self) -> u64 {
            1_000
        }
        fn local_clock(&self) -> String {
            "12:00".into()
        }
        fn publish(&mut self, view: &DiscordView) {
            self.views.push(view.clone());
        }
        fn open(&mut self, link: &str) {
            self.opened.push(link.to_owned());
        }
    }

    fn credentials() -> Credentials {
        Credentials {
            client_id: "app".into(),
            client_secret: "secret".into(),
            redirect_uri: "http://localhost".into(),
        }
    }

    fn event(evt: &str, data: &Value) -> Incoming<Command> {
        frame(&json!({"cmd": "DISPATCH", "evt": evt, "data": data}))
    }

    #[test]
    fn follows_a_call_and_notifications() {
        let channel = json!({
            "id": "50", "name": "late-night", "guild_id": "9",
            "voice_states": [
                {"user": {"id": "1", "username": "me"}, "voice_state": {}},
                {"user": {"id": "2", "username": "sam"}, "voice_state": {"self_mute": true}}
            ]
        });
        let mut fake = Fake::new(&[
            (
                "AUTHENTICATE",
                json!({"user": {"id": "1", "username": "me"}}),
            ),
            ("GET_SELECTED_VOICE_CHANNEL", channel),
            ("GET_GUILD", json!({"name": "Workbench"})),
            (
                "GET_GUILDS",
                json!({"guilds": [{"id": "9", "name": "Workbench"}]}),
            ),
            ("GET_CHANNEL", json!({"guild_id": "9"})),
        ]);
        fake.incoming
            .push_back(event("SPEAKING_START", &json!({"user_id": "2"})));
        let note = json!({
            "channel_id": "77",
            "title": "sam",
            "body": "hey",
            "message": {"id": "88"}
        });
        fake.incoming.push_back(event("NOTIFICATION_CREATE", &note));
        fake.incoming.push_back(Incoming::Command(Command::Open {
            channel_id: "77".into(),
            message_id: Some("88".into()),
        }));
        fake.incoming.push_back(Incoming::Command(Command::Stop));
        let mut world = World {
            token: Some(Token {
                access_token: "a".into(),
                refresh_token: "r".into(),
                expires_at: 100_000,
            }),
            ..World::default()
        };
        let creds = credentials();
        let end = Session::new(fake, &mut world, &creds).run(None);
        assert_eq!(end, End::Stopped);

        let last = world.views.last().unwrap();
        assert_eq!(last.status, Status::Ready);
        let voice = last.voice.as_ref().unwrap();
        assert_eq!(
            (voice.channel.as_str(), voice.guild.as_str()),
            ("late-night", "Workbench")
        );
        assert_eq!(voice.members.len(), 2);
        assert!(voice.members[1].speaking && voice.members[1].mute);
        assert_eq!(voice.members[1].join, 2);
        assert_eq!(last.notifications[0].body, "hey");
        assert_eq!(world.opened, ["discord://-/channels/9/77/88"]);
    }

    #[test]
    fn refreshes_an_expiring_token() {
        let fake = Fake::new(&[("AUTHENTICATE", json!({"user": {"id": "1"}}))]);
        let mut world = World {
            token: Some(Token {
                access_token: "old".into(),
                refresh_token: "r".into(),
                expires_at: 1_100, // within the 5-minute margin
            }),
            ..World::default()
        };
        let creds = credentials();
        // The script ends after sign-in, which ends the session.
        let end = Session::new(fake, &mut world, &creds).run(None);
        assert!(matches!(end, End::Failed(RpcError::Closed(_))));
        assert_eq!(world.token.unwrap().access_token, "new");
    }

    #[test]
    fn ignores_opening_unknown_channels() {
        let mut fake =
            Fake::new(&[("AUTHENTICATE", json!({"user": {"id": "1"}}))]);
        fake.incoming.push_back(Incoming::Command(Command::Open {
            channel_id: "123".into(),
            message_id: None,
        }));
        fake.incoming.push_back(Incoming::Command(Command::Stop));
        let mut world = World {
            token: Some(Token {
                access_token: "a".into(),
                refresh_token: "r".into(),
                expires_at: 100_000,
            }),
            ..World::default()
        };
        let creds = credentials();
        let _ = Session::new(fake, &mut world, &creds).run(None);
        assert_eq!(world.opened, Vec::<String>::new());
    }
}
