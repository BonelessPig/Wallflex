//! What the Discord widget shows, and the conversions from Discord's RPC
//! JSON into it. Ported from the user's `DiscordBridge` (`member_from`,
//! `avatar_url`, `add_notification`).

use std::collections::VecDeque;

use serde_json::Value;

/// Discord's image CDN.
const CDN: &str = "https://cdn.discordapp.com";
/// Discord's default avatars come in this many colours.
const DEFAULT_AVATARS: u64 = 6;
/// A user id's timestamp part starts this many bits up.
const SNOWFLAKE_TIME_SHIFT: u32 = 22;
/// Notifications kept, newest first.
pub const NOTIFICATION_LIMIT: usize = 20;
/// Shown for a call outside any server.
pub const DIRECT_CALL: &str = "DIRECT CALL";

/// Everything the widget draws, published by the session thread.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DiscordView {
    /// Whether the session is up, and if not, why.
    pub status: Status,
    /// The signed-in user's name.
    pub user: Option<String>,
    /// The call you're in.
    pub voice: Option<Voice>,
    /// Recent notifications, newest first.
    pub notifications: VecDeque<Notification>,
    /// Your servers, for the SERVER page's list.
    pub servers: Vec<Guild>,
    /// Voice channels of the server the SERVER page asked for.
    pub server: Option<ServerView>,
}

/// The session's state.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Status {
    /// Not connected; the reason, for the log and tooltip.
    #[default]
    Offline,
    /// Waiting for the user to approve Wallflex in Discord.
    Authorizing,
    /// Connected and signed in.
    Ready,
}

/// A voice call.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Voice {
    /// Channel id.
    pub channel_id: String,
    /// Channel name.
    pub channel: String,
    /// Server name, or [`DIRECT_CALL`].
    pub guild: String,
    /// Server id; `None` for a DM call.
    pub guild_id: Option<String>,
    /// Who's in it, in join order.
    pub members: Vec<Member>,
}

/// A person in a voice channel.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "independent flags, one per field of Discord's voice state"
)]
pub struct Member {
    /// User id.
    pub id: String,
    /// Display name.
    pub name: String,
    /// Avatar URL.
    pub avatar: String,
    /// Whether it's you.
    pub is_self: bool,
    /// Join order in the current call, rising.
    pub join: u64,
    /// Talking right now.
    pub speaking: bool,
    /// Muted (self, server or suppressed).
    pub mute: bool,
    /// Deafened.
    pub deaf: bool,
    /// Camera on (only known through the bot).
    pub video: bool,
    /// Screen sharing (only known through the bot).
    pub live: bool,
    /// Online status from the bot (`online`, `idle`, `dnd`,
    /// `streaming`); empty when unknown.
    pub status: String,
}

/// A notification.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Notification {
    /// Rising number, for "unread since".
    pub seq: u64,
    /// Local time it arrived, `HH:MM`.
    pub clock: String,
    /// Usually the sender.
    pub title: String,
    /// The message.
    pub body: String,
    /// Channel to open it in.
    pub channel_id: Option<String>,
    /// That channel's server; `None` for DMs.
    pub guild_id: Option<String>,
    /// The message, to scroll to.
    pub message_id: Option<String>,
}

/// One of your servers.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Guild {
    /// Server id.
    pub id: String,
    /// Server name.
    pub name: String,
    /// Icon URL.
    pub icon: Option<String>,
}

/// A server's occupied voice channels.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ServerView {
    /// Server id.
    pub id: String,
    /// Server name.
    pub name: String,
    /// Icon URL.
    pub icon: Option<String>,
    /// Voice channels with people in them, in channel order.
    pub channels: Vec<VoiceChannel>,
    /// Voice channels with nobody in them.
    pub empty: usize,
}

/// An occupied voice channel.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct VoiceChannel {
    /// Channel id.
    pub id: String,
    /// Channel name.
    pub name: String,
    /// Whether you're in it.
    pub you: bool,
    /// Who's in it.
    pub members: Vec<Member>,
}

/// A string field of a JSON object, if present.
pub fn text<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    value
        .get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
}

/// An owned string field, if present.
pub fn owned(value: &Value, key: &str) -> Option<String> {
    text(value, key).map(str::to_owned)
}

/// A boolean field; missing counts as false.
fn flag(value: &Value, key: &str) -> bool {
    value.get(key).and_then(Value::as_bool).unwrap_or(false)
}

/// A user's avatar URL, or Discord's default avatar for them.
pub fn avatar_url(user: &Value) -> String {
    let id = text(user, "id").unwrap_or("0");
    if let Some(hash) = text(user, "avatar") {
        return format!("{CDN}/avatars/{id}/{hash}.png?size=64");
    }
    let index = id
        .parse::<u64>()
        .ok()
        .map_or(0, |n| (n >> SNOWFLAKE_TIME_SHIFT) % DEFAULT_AVATARS);
    format!("{CDN}/embed/avatars/{index}.png")
}

/// A user's display name: server nickname, global name or username.
pub fn display_name(entry: &Value, user: &Value) -> String {
    text(entry, "nick")
        .or_else(|| text(user, "global_name"))
        .or_else(|| text(user, "username"))
        .unwrap_or("?")
        .to_owned()
}

/// A member from an RPC voice state entry. `None` without a user id.
pub fn member_from(entry: &Value, self_id: Option<&str>) -> Option<Member> {
    let user = entry.get("user").unwrap_or(&Value::Null);
    let voice = entry.get("voice_state").unwrap_or(&Value::Null);
    let id = text(user, "id")?.to_owned();
    Some(Member {
        is_self: self_id == Some(id.as_str()),
        name: display_name(entry, user),
        avatar: avatar_url(user),
        join: 0,
        speaking: false,
        mute: flag(voice, "self_mute")
            || flag(voice, "mute")
            || flag(voice, "suppress")
            || flag(entry, "mute"),
        deaf: flag(voice, "self_deaf") || flag(voice, "deaf"),
        video: flag(voice, "self_video"),
        live: flag(voice, "self_stream"),
        status: String::new(),
        id,
    })
}

/// A notification from a `NOTIFICATION_CREATE` event.
pub fn notification_from(
    data: &Value,
    guild_id: Option<String>,
    seq: u64,
    clock: String,
) -> Notification {
    let message = data.get("message").unwrap_or(&Value::Null);
    let author = message.get("author").unwrap_or(&Value::Null);
    Notification {
        seq,
        clock,
        title: owned(data, "title")
            .or_else(|| owned(author, "username"))
            .unwrap_or_default(),
        body: owned(data, "body")
            .or_else(|| owned(message, "content"))
            .unwrap_or_default(),
        channel_id: owned(data, "channel_id"),
        guild_id,
        message_id: owned(message, "id"),
    }
}

/// `text` ready to draw with GDI: invisible formatting characters
/// removed (Discord wraps names in direction isolates, which GDI draws as
/// boxes), and custom emoji (`<:name:id>`, `<a:name:id>`) as `:name:`.
pub fn clean_text(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find('<') {
        let (before, tag) = rest.split_at(start);
        push_visible(&mut out, before);
        if let Some((name, len)) = custom_emoji(tag) {
            out.push(':');
            out.push_str(name);
            out.push(':');
            rest = tag.get(len..).unwrap_or_default();
        } else {
            out.push('<');
            rest = tag.get(1..).unwrap_or_default();
        }
    }
    push_visible(&mut out, rest);
    out
}

/// Appends `text` without its invisible formatting characters.
fn push_visible(out: &mut String, text: &str) {
    out.extend(text.chars().filter(|c| !is_invisible(*c)));
}

/// Zero-width and direction-control characters, and emoji variation
/// selectors: GDI can't draw them invisibly.
fn is_invisible(c: char) -> bool {
    matches!(
        c,
        '\u{200B}'..='\u{200F}'
            | '\u{202A}'..='\u{202E}'
            | '\u{2060}'..='\u{2069}'
            | '\u{FE0E}'
            | '\u{FE0F}'
            | '\u{FEFF}'
    )
}

/// If `tag` starts with a custom emoji (`<:name:id>` or `<a:name:id>`),
/// its name and length in bytes.
fn custom_emoji(tag: &str) -> Option<(&str, usize)> {
    let end = tag.find('>')?;
    let inner = tag.get(1..end)?;
    let inner = inner.strip_prefix('a').unwrap_or(inner);
    let inner = inner.strip_prefix(':')?;
    let (name, id) = inner.split_once(':')?;
    let named = !name.is_empty()
        && name.chars().all(|c| c.is_alphanumeric() || c == '_');
    (named && is_id(id)).then(|| (name, end.saturating_add(1)))
}

/// A `discord://` link that opens `channel` (and scrolls to `message`).
pub fn discord_link(
    guild_id: Option<&str>,
    channel_id: &str,
    message_id: Option<&str>,
) -> String {
    let guild = guild_id.unwrap_or("@me");
    let mut link = format!("discord://-/channels/{guild}/{channel_id}");
    if let Some(message) = message_id {
        link.push('/');
        link.push_str(message);
    }
    link
}

/// Whether `text` is a Discord id (only digits), safe to put in a link.
pub fn is_id(text: &str) -> bool {
    !text.is_empty() && text.bytes().all(|b| b.is_ascii_digit())
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{
        avatar_url, discord_link, is_id, member_from, notification_from,
    };

    #[test]
    fn avatar_urls() {
        let with_hash = json!({"id": "123", "avatar": "abc"});
        assert_eq!(
            avatar_url(&with_hash),
            "https://cdn.discordapp.com/avatars/123/abc.png?size=64"
        );
        // Default avatar: (id >> 22) % 6.
        let id = (5_u64 << 22).to_string();
        let without = json!({ "id": id });
        assert_eq!(
            avatar_url(&without),
            "https://cdn.discordapp.com/embed/avatars/5.png"
        );
    }

    #[test]
    fn members_from_voice_states() {
        let entry = json!({
            "nick": "Nick",
            "mute": false,
            "user": {"id": "42", "username": "user", "global_name": "Global"},
            "voice_state": {"self_mute": true, "self_deaf": false}
        });
        let member = member_from(&entry, Some("42")).unwrap();
        assert_eq!(member.name, "Nick");
        assert!(member.is_self && member.mute && !member.deaf);

        let plain = json!({"user": {"id": "7", "username": "user"}});
        let member = member_from(&plain, Some("42")).unwrap();
        assert_eq!(member.name, "user");
        assert!(!member.is_self && !member.mute);

        assert_eq!(member_from(&json!({"user": {}}), None), None);
    }

    #[test]
    fn notifications() {
        let data = json!({
            "channel_id": "100",
            "message": {"id": "200", "content": "hi", "author": {"username": "sam"}}
        });
        let note =
            notification_from(&data, Some("9".into()), 3, "12:34".into());
        assert_eq!(note.title, "sam");
        assert_eq!(note.body, "hi");
        assert_eq!(note.channel_id.as_deref(), Some("100"));
        assert_eq!(note.message_id.as_deref(), Some("200"));
        assert_eq!(note.seq, 3);
    }

    #[test]
    fn cleans_text_for_drawing() {
        use super::clean_text;
        let isolated = "Shrimp \u{2068}(#general\u{2069})";
        assert_eq!(clean_text(isolated), "Shrimp (#general)");
        assert_eq!(
            clean_text("gg <:pog:123> <a:dance:45>!"),
            "gg :pog: :dance:!"
        );
        assert_eq!(clean_text("a < b <:x:y> <@1>"), "a < b <:x:y> <@1>");
        assert_eq!(clean_text("\u{2764}\u{FE0F}"), "\u{2764}");
    }

    #[test]
    fn links() {
        assert_eq!(
            discord_link(Some("1"), "2", Some("3")),
            "discord://-/channels/1/2/3"
        );
        assert_eq!(discord_link(None, "2", None), "discord://-/channels/@me/2");
        assert!(is_id("123"));
        assert!(!is_id("12a") && !is_id(""));
    }
}
