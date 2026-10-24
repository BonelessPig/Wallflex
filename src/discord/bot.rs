//! The optional bot: who's online in your servers, friends' activity,
//! and camera / screen-share state (which the local RPC can't see).
//! Ported from the user's `discord_bot.py`, which used discord.py; here
//! the gateway protocol and member cache are handled directly.

use std::collections::HashMap;

use serde_json::{Value, json};

use super::model::{avatar_url, owned, text};

/// Gateway intents: guilds, members, voice states, presences.
pub const INTENTS: u64 = (1 << 0) | (1 << 1) | (1 << 7) | (1 << 8);
/// Online members listed per server.
const IDLE_LIMIT: usize = 40;
/// Gateway opcodes.
const OP_DISPATCH: u64 = 0;
/// Heartbeat (sent, or requested by Discord).
const OP_HEARTBEAT: u64 = 1;
/// Identify.
const OP_IDENTIFY: u64 = 2;
/// Request guild members.
const OP_REQUEST_MEMBERS: u64 = 8;
/// Reconnect.
const OP_RECONNECT: u64 = 7;
/// Invalid session.
const OP_INVALID_SESSION: u64 = 9;
/// Hello.
const OP_HELLO: u64 = 10;
/// Close code: the token was rejected.
pub const CLOSE_AUTH_FAILED: u16 = 4004;
/// Close code: the intents aren't enabled for the bot.
pub const CLOSE_DISALLOWED_INTENTS: u16 = 4014;
/// Activity types, as Discord numbers them (0, playing, is the
/// default). Streaming.
const STREAMING: u64 = 1;
/// Listening.
const LISTENING: u64 = 2;
/// Watching.
const WATCHING: u64 = 3;
/// A custom status.
const CUSTOM: u64 = 4;
/// Competing.
const COMPETING: u64 = 5;

/// Someone, as the widget shows them.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Person {
    /// User id.
    pub id: String,
    /// Display name.
    pub name: String,
    /// Avatar URL.
    pub avatar: String,
    /// `online`, `idle`, `dnd` or `streaming`.
    pub status: String,
}

/// A friend who's online.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Friend {
    /// Who.
    pub person: Person,
    /// What they're doing ("Playing ...").
    pub activity: Option<String>,
    /// The voice channel and server they're in.
    pub voice: Option<(String, String)>,
}

/// One server's online counts.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ServerOnline {
    /// Online people.
    pub online: usize,
    /// Members in total.
    pub total: u64,
    /// Online people not in voice, by status then name.
    pub idle: Vec<Person>,
}

/// Where the bot is.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum BotStatus {
    /// Connecting.
    #[default]
    Connecting,
    /// Connected and the cache is filled.
    Ready,
    /// Stopped, with the reason to show.
    Failed(String),
}

/// What the bot adds to the widget.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BotView {
    /// The bot's state.
    pub status: BotStatus,
    /// Online counts by server id.
    pub servers: HashMap<String, ServerOnline>,
    /// Friends online, in the config's order.
    pub friends: Vec<Friend>,
    /// Friends who are offline.
    pub offline: usize,
    /// Friend ids in no server the bot shares.
    pub missing: usize,
    /// Camera and screen share by user id, for people in voice.
    pub voice_flags: HashMap<String, (bool, bool)>,
}

/// Looking a friend up.
enum Lookup {
    /// In no server the bot shares.
    Missing,
    /// Found, but offline.
    Offline,
    /// Online.
    Online(Friend),
}

/// A cached member.
#[derive(Clone, Debug, Default)]
struct MemberInfo {
    /// Display name.
    name: String,
    /// Avatar URL.
    avatar: String,
    /// A bot account.
    bot: bool,
}

/// A cached presence.
#[derive(Clone, Debug, Default)]
struct Presence {
    /// `online`, `idle` or `dnd`.
    status: String,
    /// Live right now.
    streaming: bool,
    /// What they're doing.
    activity: Option<String>,
}

/// A cached voice state.
#[derive(Clone, Debug, Default)]
struct VoiceInfo {
    /// The channel.
    channel_id: String,
    /// Camera on.
    video: bool,
    /// Screen sharing.
    live: bool,
}

/// Everything known about one server.
#[derive(Default)]
struct GuildCache {
    /// Server name.
    name: String,
    /// Member count Discord reports.
    member_count: u64,
    /// Members by user id.
    members: HashMap<String, MemberInfo>,
    /// Presences of online members by user id.
    presences: HashMap<String, Presence>,
    /// Voice states by user id.
    voice: HashMap<String, VoiceInfo>,
    /// Channel names by id.
    channels: HashMap<String, String>,
}

/// What the connection should do after a gateway message.
#[derive(Clone, Debug, PartialEq)]
pub enum Reply {
    /// Start heartbeating every this many milliseconds and identify.
    Hello(u64),
    /// Send this payload.
    Send(Value),
    /// Drop the connection and start again.
    Reconnect,
}

/// The gateway's state and the member cache.
pub struct Bot {
    /// Bot token.
    token: String,
    /// Friend ids from the config.
    friends: Vec<String>,
    /// Servers by id.
    guilds: HashMap<String, GuildCache>,
    /// Last sequence number, for heartbeats.
    sequence: Option<u64>,
    /// Whether READY arrived.
    ready: bool,
}

impl Bot {
    /// A bot for `token`, following `friends`.
    pub fn new(token: String, friends: Vec<String>) -> Self {
        Self {
            token,
            friends,
            guilds: HashMap::new(),
            sequence: None,
            ready: false,
        }
    }

    /// Forgets everything, for a fresh connection.
    pub fn reset(&mut self) {
        self.guilds.clear();
        self.sequence = None;
        self.ready = false;
    }

    /// The heartbeat payload.
    pub fn heartbeat(&self) -> Value {
        json!({"op": OP_HEARTBEAT, "d": self.sequence})
    }

    /// The identify payload: invisible, so the bot doesn't show up as
    /// online in member lists.
    pub fn identify(&self) -> Value {
        json!({
            "op": OP_IDENTIFY,
            "d": {
                "token": self.token,
                "intents": INTENTS,
                "properties": {"os": "windows", "browser": "wallflex", "device": "wallflex"},
                "presence": {"status": "invisible", "since": null, "activities": [], "afk": false}
            }
        })
    }

    /// Handles one gateway message. Returns what to send or do, and
    /// whether the view changed (`true`) or only acks arrived.
    pub fn handle(&mut self, message: &Value) -> (Vec<Reply>, bool) {
        if let Some(sequence) = message.get("s").and_then(Value::as_u64) {
            self.sequence = Some(sequence);
        }
        let data = message.get("d").unwrap_or(&Value::Null);
        match message.get("op").and_then(Value::as_u64) {
            Some(OP_HELLO) => {
                let interval = data
                    .get("heartbeat_interval")
                    .and_then(Value::as_u64)
                    .unwrap_or(41_250);
                (vec![Reply::Hello(interval)], false)
            }
            Some(OP_HEARTBEAT) => (vec![Reply::Send(self.heartbeat())], false),
            Some(OP_RECONNECT | OP_INVALID_SESSION) => {
                (vec![Reply::Reconnect], false)
            }
            Some(OP_DISPATCH) => {
                let event = text(message, "t").unwrap_or("");
                (self.dispatch(event, data), true)
            }
            // Heartbeat acks and anything new need no reply.
            _ => (Vec::new(), false),
        }
    }

    /// Applies a dispatched event.
    fn dispatch(&mut self, event: &str, data: &Value) -> Vec<Reply> {
        let guild_id = owned(data, "guild_id");
        match event {
            "READY" => self.ready = true,
            "GUILD_CREATE" => return self.guild_create(data),
            "GUILD_DELETE" => {
                if let Some(id) = text(data, "id") {
                    self.guilds.remove(id);
                }
            }
            "GUILD_MEMBERS_CHUNK"
            | "GUILD_MEMBER_ADD"
            | "GUILD_MEMBER_UPDATE" => {
                if let Some(guild) =
                    guild_id.and_then(|id| self.guilds.get_mut(&id))
                {
                    let members = data.get("members").and_then(Value::as_array);
                    let single = [data.clone()];
                    let list = members.map_or(single.as_slice(), Vec::as_slice);
                    for member in list {
                        add_member(guild, member);
                    }
                    for presence in array(data, "presences") {
                        set_presence(guild, presence);
                    }
                }
            }
            "GUILD_MEMBER_REMOVE" => {
                let user = data.get("user").unwrap_or(&Value::Null);
                if let (Some(guild), Some(id)) = (
                    guild_id.and_then(|g| self.guilds.get_mut(&g)),
                    text(user, "id"),
                ) {
                    guild.members.remove(id);
                    guild.presences.remove(id);
                }
            }
            "PRESENCE_UPDATE" => {
                if let Some(guild) =
                    guild_id.and_then(|id| self.guilds.get_mut(&id))
                {
                    set_presence(guild, data);
                }
            }
            "VOICE_STATE_UPDATE" => {
                if let Some(guild) =
                    guild_id.and_then(|id| self.guilds.get_mut(&id))
                {
                    set_voice(guild, data);
                }
            }
            "CHANNEL_CREATE" | "CHANNEL_UPDATE" => {
                if let (Some(guild), Some(id), Some(name)) = (
                    guild_id.and_then(|g| self.guilds.get_mut(&g)),
                    owned(data, "id"),
                    owned(data, "name"),
                ) {
                    guild.channels.insert(id, name);
                }
            }
            _ => {}
        }
        Vec::new()
    }

    /// Fills a server's cache; asks for the full member list when the
    /// event only carried some of it.
    fn guild_create(&mut self, data: &Value) -> Vec<Reply> {
        let Some(id) = owned(data, "id") else {
            return Vec::new();
        };
        let mut guild = GuildCache {
            name: owned(data, "name").unwrap_or_default(),
            member_count: data
                .get("member_count")
                .and_then(Value::as_u64)
                .unwrap_or(0),
            ..GuildCache::default()
        };
        for member in array(data, "members") {
            add_member(&mut guild, member);
        }
        for presence in array(data, "presences") {
            set_presence(&mut guild, presence);
        }
        for state in array(data, "voice_states") {
            set_voice(&mut guild, state);
        }
        for channel in array(data, "channels") {
            if let (Some(channel_id), Some(name)) =
                (owned(channel, "id"), owned(channel, "name"))
            {
                guild.channels.insert(channel_id, name);
            }
        }
        let partial = u64::try_from(guild.members.len()).unwrap_or(u64::MAX)
            < guild.member_count;
        self.guilds.insert(id.clone(), guild);
        if partial {
            let request = json!({
                "op": OP_REQUEST_MEMBERS,
                "d": {"guild_id": id, "query": "", "limit": 0, "presences": true}
            });
            return vec![Reply::Send(request)];
        }
        Vec::new()
    }

    /// The view for the widget.
    pub fn view(&self) -> BotView {
        let mut view = BotView {
            status: if self.ready {
                BotStatus::Ready
            } else {
                BotStatus::Connecting
            },
            ..BotView::default()
        };
        for (id, guild) in &self.guilds {
            view.servers.insert(id.clone(), server_online(guild));
            for (user, voice) in &guild.voice {
                view.voice_flags
                    .insert(user.clone(), (voice.video, voice.live));
            }
        }
        for friend in &self.friends {
            match self.find_friend(friend) {
                Lookup::Missing => {
                    view.missing = view.missing.saturating_add(1);
                }
                Lookup::Offline => {
                    view.offline = view.offline.saturating_add(1);
                }
                Lookup::Online(found) => view.friends.push(found),
            }
        }
        view
    }

    /// Looks a friend up, preferring a server they're in voice in
    /// (presence is the same in every server).
    fn find_friend(&self, id: &str) -> Lookup {
        let mut found = Lookup::Missing;
        for guild in self.guilds.values() {
            let Some(member) = guild.members.get(id) else {
                continue;
            };
            let in_voice = guild.voice.get(id);
            let friend = guild.presences.get(id).map(|presence| Friend {
                person: person(id, member, presence),
                activity: presence.activity.clone(),
                voice: in_voice.map(|v| {
                    let channel = guild
                        .channels
                        .get(&v.channel_id)
                        .cloned()
                        .unwrap_or_default();
                    (channel, guild.name.clone())
                }),
            });
            let lookup = friend.map_or(Lookup::Offline, Lookup::Online);
            if in_voice.is_some() {
                return lookup;
            }
            if matches!(found, Lookup::Missing) {
                found = lookup;
            }
        }
        found
    }
}

/// The items of a JSON array field.
fn array<'a>(value: &'a Value, key: &str) -> impl Iterator<Item = &'a Value> {
    value
        .get(key)
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
}

/// Adds a member from a guild member object.
fn add_member(guild: &mut GuildCache, member: &Value) {
    let user = member.get("user").unwrap_or(&Value::Null);
    let Some(id) = owned(user, "id") else {
        return;
    };
    let name = text(user, "global_name")
        .or_else(|| text(user, "username"))
        .unwrap_or("?");
    guild.members.insert(
        id,
        MemberInfo {
            name: name.to_owned(),
            avatar: avatar_url(user),
            bot: user.get("bot").and_then(Value::as_bool).unwrap_or(false),
        },
    );
}

/// Stores (or, for offline, removes) a presence.
fn set_presence(guild: &mut GuildCache, presence: &Value) {
    let user = presence.get("user").unwrap_or(&Value::Null);
    let Some(id) = owned(user, "id") else {
        return;
    };
    let status = text(presence, "status").unwrap_or("offline");
    if status == "offline" || status == "invisible" {
        guild.presences.remove(&id);
        return;
    }
    let activities: Vec<&Value> = array(presence, "activities").collect();
    let streaming = activities
        .iter()
        .any(|a| a.get("type").and_then(Value::as_u64) == Some(STREAMING));
    guild.presences.insert(
        id,
        Presence {
            status: status.to_owned(),
            streaming,
            activity: activity_text(&activities),
        },
    );
}

/// Stores a voice state (a null channel means they left).
fn set_voice(guild: &mut GuildCache, state: &Value) {
    let Some(user) = owned(state, "user_id") else {
        return;
    };
    match owned(state, "channel_id") {
        Some(channel_id) => {
            let flag =
                |key| state.get(key).and_then(Value::as_bool).unwrap_or(false);
            guild.voice.insert(
                user,
                VoiceInfo {
                    channel_id,
                    video: flag("self_video"),
                    live: flag("self_stream"),
                },
            );
        }
        None => {
            guild.voice.remove(&user);
        }
    }
}

/// A short line for what someone's doing, as discord.py's version did.
fn activity_text(activities: &[&Value]) -> Option<String> {
    let mut custom = None;
    for activity in activities {
        let kind = activity.get("type").and_then(Value::as_u64);
        let name = text(activity, "name");
        if name == Some("Spotify") && kind == Some(LISTENING) {
            let title = text(activity, "details").unwrap_or("");
            return Some(format!("Spotify · {title}"));
        }
        if kind == Some(CUSTOM) {
            custom = custom.or_else(|| owned(activity, "state"));
            continue;
        }
        let Some(name) = name else {
            continue;
        };
        return Some(match kind {
            Some(STREAMING) => format!("Streaming {name}"),
            Some(LISTENING) => format!("Listening to {name}"),
            Some(WATCHING) => format!("Watching {name}"),
            Some(COMPETING) => format!("Competing in {name}"),
            // Playing, and anything Discord adds later.
            _ => format!("Playing {name}"),
        });
    }
    custom
}

/// A person from the cache.
fn person(id: &str, member: &MemberInfo, presence: &Presence) -> Person {
    Person {
        id: id.to_owned(),
        name: member.name.clone(),
        avatar: member.avatar.clone(),
        status: if presence.streaming {
            "streaming".to_owned()
        } else {
            presence.status.clone()
        },
    }
}

/// Sort rank of a status: streaming, online, idle, do not disturb.
pub fn status_rank(status: &str) -> u8 {
    match status {
        "streaming" => 0,
        "online" => 1,
        "idle" => 2,
        _ => 3,
    }
}

/// A server's online count and online people not in voice.
fn server_online(guild: &GuildCache) -> ServerOnline {
    let mut online: Vec<Person> = guild
        .presences
        .iter()
        .filter_map(|(id, presence)| {
            let member = guild.members.get(id).filter(|m| !m.bot)?;
            Some(person(id, member, presence))
        })
        .collect();
    let count = online.len();
    online.retain(|p| !guild.voice.contains_key(&p.id));
    online.sort_by(|a, b| {
        status_rank(&a.status)
            .cmp(&status_rank(&b.status))
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    online.truncate(IDLE_LIMIT);
    ServerOnline {
        online: count,
        total: guild.member_count,
        idle: online,
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{Bot, BotStatus, Reply};

    fn dispatch(t: &str, d: &serde_json::Value) -> serde_json::Value {
        json!({"op": 0, "s": 1, "t": t, "d": d})
    }

    fn guild() -> serde_json::Value {
        json!({
            "id": "9", "name": "Workbench", "member_count": 3,
            "channels": [{"id": "50", "name": "late-night", "type": 2}],
            "members": [
                {"user": {"id": "1", "username": "sam"}},
                {"user": {"id": "2", "username": "riley", "global_name": "Riley"}},
                {"user": {"id": "3", "username": "bot", "bot": true}}
            ],
            "presences": [
                {
                    "user": {"id": "1"},
                    "status": "online",
                    "activities": [{"type": 0, "name": "Factorio"}]
                },
                {"user": {"id": "2"}, "status": "idle", "activities": []},
                {"user": {"id": "3"}, "status": "online", "activities": []}
            ],
            "voice_states": [{"user_id": "2", "channel_id": "50", "self_video": true}]
        })
    }

    #[test]
    fn hello_heartbeat_and_reconnect() {
        let mut bot = Bot::new("t".into(), vec![]);
        let (replies, _) =
            bot.handle(&json!({"op": 10, "d": {"heartbeat_interval": 1000}}));
        assert_eq!(replies, vec![Reply::Hello(1000)]);
        let (replies, _) = bot.handle(&json!({"op": 7}));
        assert_eq!(replies, vec![Reply::Reconnect]);
        assert_eq!(bot.heartbeat(), json!({"op": 1, "d": null}));
        assert_eq!(bot.identify()["d"]["intents"], json!(387));
    }

    #[test]
    fn builds_server_and_friends_views() {
        let mut bot =
            Bot::new("t".into(), vec!["1".into(), "2".into(), "77".into()]);
        bot.handle(&dispatch("READY", &json!({})));
        bot.handle(&dispatch("GUILD_CREATE", &guild()));
        let view = bot.view();
        assert_eq!(view.status, BotStatus::Ready);
        let server = &view.servers["9"];
        assert_eq!(
            (server.online, server.total),
            (2, 3),
            "the bot isn't counted"
        );
        assert_eq!(server.idle.len(), 1, "Riley is in voice");
        assert_eq!(server.idle[0].name, "sam");
        assert_eq!(view.friends.len(), 2);
        assert_eq!(
            view.friends[0].activity.as_deref(),
            Some("Playing Factorio")
        );
        assert_eq!(
            view.friends[1].voice,
            Some(("late-night".into(), "Workbench".into()))
        );
        assert_eq!(view.missing, 1);
        assert_eq!(view.voice_flags["2"], (true, false));
    }

    #[test]
    fn presence_and_voice_updates() {
        let mut bot = Bot::new("t".into(), vec!["1".into()]);
        bot.handle(&dispatch("GUILD_CREATE", &guild()));
        bot.handle(&dispatch(
            "PRESENCE_UPDATE",
            &json!({"guild_id": "9", "user": {"id": "1"}, "status": "offline"}),
        ));
        assert_eq!(bot.view().offline, 1);
        bot.handle(&dispatch(
            "VOICE_STATE_UPDATE",
            &json!({"guild_id": "9", "user_id": "2", "channel_id": null}),
        ));
        assert!(!bot.view().voice_flags.contains_key("2"));
    }

    #[test]
    fn asks_for_missing_members() {
        let mut bot = Bot::new("t".into(), vec![]);
        let mut big = guild();
        big["member_count"] = json!(500);
        let (replies, _) = bot.handle(&dispatch("GUILD_CREATE", &big));
        assert!(matches!(&replies[..], [Reply::Send(v)] if v["op"] == 8));
    }

    #[test]
    fn activity_lines() {
        let spotify = json!({"type": 2, "name": "Spotify", "details": "Song"});
        let custom =
            json!({"type": 4, "name": "Custom Status", "state": "brb"});
        assert_eq!(
            super::activity_text(&[&spotify]).as_deref(),
            Some("Spotify · Song")
        );
        assert_eq!(super::activity_text(&[&custom]).as_deref(), Some("brb"));
        let watching = json!({"type": 3, "name": "a film"});
        assert_eq!(
            super::activity_text(&[&custom, &watching]).as_deref(),
            Some("Watching a film")
        );
    }
}
