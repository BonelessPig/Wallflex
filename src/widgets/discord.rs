//! The Discord widget, redrawn from the user's wallpaper (`discord.js`
//! and its CSS): tabs, the VOICE page (your call), INBOX (recent
//! notifications), a one-line summary when collapsed, and a crossed-out
//! icon while Discord is offline.

mod friends;
mod server;

use std::collections::{HashMap, HashSet};

use super::{Align, INK, Pen, QUIET, TRACK};
use crate::canvas::{Area, Paint};
use crate::color::Rgb;
use crate::discord::model::{
    DiscordView, Member, Notification, Status, Voice, clean_text,
};
use crate::discord::session::Command;
use crate::image::Image;

/// Widget width (24vh).
pub const WIDTH: f64 = 240.0;
/// Text size (1.05vh).
const TEXT: f64 = 10.5;
/// Height of a line of text.
const LINE: f64 = 13.0;
/// Baseline within a line.
const BASELINE: f64 = 10.0;
/// Gap between body rows (0.7vh).
const GAP: f64 = 7.0;
/// Height of the tab row, underline included.
const TABS_H: f64 = 16.0;
/// Gap under the tabs (1vh).
const TABS_GAP: f64 = 10.0;
/// Gap between tabs (1.2vh).
const TAB_GAP: f64 = 12.0;
/// Picture size in avatar rows (2.8vh).
const FACE: f64 = 28.0;
/// Small picture size (1.9vh).
const FACE_SMALL: f64 = 19.0;
/// Gap between pictures in a row (0.9vh), and in a small row (0.7vh).
const AVATAR_GAP: (f64, f64) = (9.0, 7.0);
/// People per avatar line before wrapping.
const AVATARS_PER_LINE: usize = 10;
/// People with a name row in the names view.
const MAX_NAMED: usize = 6;
/// Notifications shown on INBOX.
const INBOX_ROWS: usize = 4;
/// Gap between a notification's two lines.
const NOTE_LINE_GAP: f64 = 3.0;
/// Friends remembered as clicked.
const MAX_FRIEND_TOP: usize = 20;
/// Status dot colours.
const IDLE: Rgb = rgb(0xFF, 0xD3, 0xA1);
/// Do not disturb.
const DND: Rgb = rgb(0xFF, 0xB0, 0xB8);
/// Streaming (dark purple).
const STREAMING: Rgb = rgb(0x4E, 0x3F, 0x73);
/// Screen sharing dot.
const LIVE: Rgb = rgb(0xFF, 0x5A, 0x5F);
/// Pictures kept; past this the cache starts again.
const MAX_IMAGES: usize = 200;
/// Room outside a picture for its speaking / muted ring (2.5 offset
/// plus 2 wide, for full-size pictures).
const RING_ROOM: f64 = 5.0;
/// Room on each side of and under the widget so rings and badges at
/// its edges aren't cut off. The content is still [`WIDTH`] wide.
const BLEED: f64 = 5.0;
/// The call view toggle's size (1.3vh), and its icon's design grid.
const VIEW_ICON: f64 = 13.0;
/// The icons' design grid (16 x 16, as the wallpaper's SVGs).
const ICON_GRID: f64 = 16.0;
/// Muted / deafened badge size (1.4vh), and on small pictures (1.1vh).
const STATE_BADGE: (f64, f64) = (14.0, 11.0);
/// How far the muted / deafened badge sticks out past the picture.
const STATE_BADGE_OUT: f64 = 3.0;
/// Points per half circle in the badge icons.
const ICON_ARC_STEPS: u32 = 10;
/// Size of the offline icon (2.4vh).
const OFFLINE_ICON: f64 = 24.0;
/// Approximate width of a character as a share of the text size, for
/// fitting text (Bahnschrift capitals).
const CHAR_WIDTH: f64 = 0.58;

/// Highlight blue (channel names, the call dot).
const BLUE: Rgb = rgb(0xA8, 0xE6, 0xFF);
/// Orange (unread counts).
const ORANGE: Rgb = rgb(0xFF, 0xD3, 0xA1);
/// Ring: speaking.
const SPEAKING: Rgb = rgb(0xB4, 0xF0, 0xCC);
/// Ring: muted.
const MUTED: Rgb = rgb(0xFF, 0xDC, 0xB5);
/// Ring: deafened.
const DEAFENED: Rgb = rgb(0xFF, 0x9A, 0xA4);
/// The muted badge's symbol (a darker shade of its ring).
const MUTED_DARK: Rgb = rgb(0x7A, 0x4D, 0x2E);
/// The deafened badge's symbol.
const DEAFENED_DARK: Rgb = rgb(0x7D, 0x2F, 0x38);
/// How much a deafened person's picture is dimmed.
const DEAFENED_DIM: f64 = 0.5;
/// A hovered notification's message (brighter than [`QUIET`]).
const NOTE_HOVER: f64 = 0.8;
/// Initials and badge backgrounds.
const BLACK: Rgb = rgb(0, 0, 0);
/// Picture colours behind the initials.
const FACE_COLORS: [Rgb; 5] = [
    rgb(0xA8, 0xE6, 0xFF),
    rgb(0xCD, 0xBD, 0xFF),
    rgb(0xFF, 0xD3, 0xA1),
    rgb(0xB4, 0xF0, 0xCC),
    rgb(0xFF, 0xB0, 0xB8),
];

/// A colour from its channels.
const fn rgb(r: u8, g: u8, b: u8) -> Rgb {
    Rgb { r, g, b }
}

/// The widget's pages.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Page {
    /// Your call.
    Voice,
    /// A server's voice channels.
    Server,
    /// Friends who are online (needs the bot).
    Friends,
    /// Recent notifications.
    Inbox,
}

impl Page {
    /// Tab label.
    fn label(self) -> &'static str {
        match self {
            Self::Voice => "VOICE",
            Self::Server => "SERVER",
            Self::Friends => "FRIENDS",
            Self::Inbox => "INBOX",
        }
    }
}

/// How the VOICE page shows the people in your call.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CallView {
    /// A row per person with their name, the rest as small pictures.
    #[default]
    Names,
    /// Just a row of pictures.
    Avatars,
}

impl CallView {
    /// The other view, which the toggle switches to.
    fn other(self) -> Self {
        match self {
            Self::Names => Self::Avatars,
            Self::Avatars => Self::Names,
        }
    }
}

/// Sample data for `--preview`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Demo {
    /// A call, names view.
    Names,
    /// A call, avatars view.
    Avatars,
    /// The INBOX page.
    Inbox,
}

/// What a click on the Discord widget does.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    /// A tab: show that page, or close if it's already open.
    Tab(Page),
    /// The tab row outside the tabs: open or close.
    Header,
    /// A notification: open Discord on it.
    OpenNote {
        /// Its channel.
        channel_id: String,
        /// Its message.
        message_id: Option<String>,
    },
    /// SERVER: follow the server of my call.
    ServerFollow,
    /// SERVER: open or close the server list.
    ServerList,
    /// SERVER: turn the server list's page.
    PickerStep(i8),
    /// SERVER: show this server.
    PickServer(String),
    /// FRIENDS: move this friend to the top.
    PromoteFriend(String),
    /// VOICE: switch between names and avatars.
    CallView,
}

/// The widget's state: the session's view plus what the wallpaper kept
/// in its own variables (page, who has a name row, what's been read).
#[derive(Default)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "independent UI flags, as the wallpaper kept in variables"
)]
pub struct DiscordUi {
    /// The session's view; `None` when Discord isn't set up.
    view: Option<DiscordView>,
    /// The page shown when open.
    page: Option<Page>,
    /// Set by a click; `None` follows the rules (open during a call).
    open: Option<bool>,
    /// Whether you were in a call at the last update.
    was_in_call: bool,
    /// Ids with a name row, top to bottom.
    named: Vec<String>,
    /// When each person last started talking (update number).
    last_spoke: HashMap<String, u64>,
    /// Who was talking at the last update.
    was_speaking: HashSet<String>,
    /// Update counter.
    updates: u64,
    /// The newest notification already seen.
    inbox_seen: u64,
    /// Downloaded pictures by URL.
    images: HashMap<String, Image>,
    /// URLs already asked for.
    asked: HashSet<String>,
    /// SERVER: a picked server; `None` follows your call's server.
    picked_server: Option<String>,
    /// SERVER: the server list is open.
    picker: bool,
    /// SERVER: the server list's page.
    picker_page: usize,
    /// Whether a bot token is set.
    bot_configured: bool,
    /// The bot's view.
    bot: Option<crate::discord::bot::BotView>,
    /// The session's view before the bot's camera/stream flags.
    rpc_view: Option<DiscordView>,
    /// FRIENDS: clicked friends, most recent first.
    friend_top: Vec<String>,
    /// Whether a server has been asked of the session yet.
    server_sent: bool,
    /// The server last asked of the session.
    sent_server: Option<String>,
    /// VOICE: names or just pictures.
    call_view: CallView,
    /// The control under the pointer, drawn highlighted.
    hover: Option<Action>,
}

/// One row of the widget, top to bottom.
enum Item<'a> {
    /// The crossed-out Discord icon.
    Offline,
    /// The tab row.
    Tabs,
    /// A line of dim text.
    Dim(&'static str),
    /// The collapsed summary line.
    Summary,
    /// The call's channel and server.
    CallHead(&'a Voice),
    /// A person with a name row.
    Named(&'a Member),
    /// A thin rule.
    Rule,
    /// A row of pictures.
    Avatars(Vec<&'a Member>, bool),
    /// A notification.
    Note(&'a Notification),
    /// A row of the SERVER page.
    Server(server::Row<'a>),
    /// A row of the FRIENDS page.
    Friends(friends::Row),
}

impl DiscordUi {
    /// Whether Discord is set up (the widget hides when it isn't).
    pub fn set_configured(&mut self, configured: bool, bot: bool) {
        self.bot_configured = configured && bot;
        if !self.bot_configured {
            self.bot = None;
        }
        if !configured {
            self.view = None;
        } else if self.view.is_none() {
            self.view = Some(DiscordView::default());
        }
    }

    /// Sample data for `--preview`: a call of eight (one talking, one
    /// muted, one deafened) and three notifications, on the page or
    /// view `demo` asks for.
    pub fn show_demo(&mut self, demo: Demo) {
        self.set_configured(true, false);
        let names = [
            "Sam", "Riley", "Jordan", "Avery", "Quinn", "Morgan", "Casey",
            "Drew",
        ];
        let members: Vec<Member> = (1_u64..)
            .zip(names)
            .map(|(join, name)| Member {
                id: join.to_string(),
                name: name.to_owned(),
                join,
                is_self: join == 1,
                mute: join == 3,
                deaf: join == 5,
                // Discord's default avatars, so the preview shows real
                // downloaded pictures for half the people.
                avatar: if join % 2 == 0 {
                    format!(
                        "https://cdn.discordapp.com/embed/avatars/{}.png",
                        join % 6
                    )
                } else {
                    String::new()
                },
                ..Member::default()
            })
            .collect();
        let mut view = DiscordView {
            status: Status::Ready,
            user: Some("Sam".into()),
            voice: Some(Voice {
                channel: "late-night-builds".into(),
                guild: "Workbench".into(),
                members: members.clone(),
                ..Voice::default()
            }),
            ..DiscordView::default()
        };
        // Newest first, as the session keeps them.
        let notes = [
            (3, "21:04", "Riley", "pushed the fix, can you check?"),
            (2, "20:51", "Jordan", "lol that's exactly what I said"),
            (1, "20:12", "Workbench #general", "Avery: new build is up"),
        ];
        for (seq, clock, title, body) in notes {
            view.notifications.push_back(Notification {
                seq,
                clock: clock.into(),
                title: title.into(),
                body: body.into(),
                channel_id: Some("1".into()),
                ..Notification::default()
            });
        }
        self.update(view.clone());
        // Then the eighth person starts talking: they move up a row.
        if let Some(voice) = view.voice.as_mut()
            && let Some(last) = voice.members.last_mut()
        {
            last.speaking = true;
        }
        self.update(view);
        match demo {
            Demo::Names => {}
            Demo::Avatars => self.call_view = CallView::Avatars,
            Demo::Inbox => {
                self.page = Some(Page::Inbox);
                self.open = Some(true);
            }
        }
    }

    /// Takes in a new view from the session.
    pub fn update(&mut self, view: DiscordView) {
        if self.view.is_none() {
            return;
        }
        self.updates = self.updates.wrapping_add(1);
        let in_call = view.voice.is_some();
        let started = self.track_speaking(view.voice.as_ref());
        if in_call != self.was_in_call {
            self.was_in_call = in_call;
            self.open = None;
            self.named.clear();
            self.last_spoke.clear();
            if in_call {
                self.page = Some(Page::Voice);
            }
        }
        if let Some(voice) = &view.voice {
            self.sync_named(&voice.members, &started);
        }
        self.rpc_view = Some(view.clone());
        self.view = Some(self.with_bot_flags(view));
        if self.is_open() && self.page() == Page::Inbox {
            self.mark_inbox_seen();
        }
    }

    /// Takes in a new view from the bot.
    pub fn update_bot(&mut self, bot: Option<crate::discord::bot::BotView>) {
        if !self.bot_configured {
            return;
        }
        self.bot = bot;
        if let Some(raw) = self.rpc_view.clone() {
            self.view = Some(self.with_bot_flags(raw));
        }
    }

    /// `view` with camera and screen-share state from the bot, which
    /// sees it where RPC doesn't.
    fn with_bot_flags(&self, mut view: DiscordView) -> DiscordView {
        let Some(bot) = &self.bot else {
            return view;
        };
        if let Some(voice) = view.voice.as_mut() {
            for member in &mut voice.members {
                if let Some(&(video, live)) = bot.voice_flags.get(&member.id) {
                    member.video |= video;
                    member.live |= live;
                }
            }
        }
        view
    }

    /// Size in design units; (0, 0) when Discord isn't set up.
    pub fn size(&self) -> (f64, f64) {
        let items = self.items();
        if items.is_empty() {
            return (0.0, 0.0);
        }
        if matches!(items.first(), Some(Item::Offline)) {
            return (OFFLINE_ICON, OFFLINE_ICON);
        }
        // Room on both sides and underneath for rings and badges.
        (WIDTH + BLEED * 2.0, Self::height(&items) + BLEED)
    }

    /// Draws the widget.
    pub fn draw(&self, pen: &mut Pen<'_, '_>) {
        let items = self.items();
        if !matches!(items.first(), Some(Item::Offline)) {
            pen.origin.0 += BLEED;
        }
        let mut y = 0.0;
        let mut previous = None;
        for item in &items {
            y += gap_between(previous, item);
            self.draw_item(pen, item, y);
            y += Self::item_height(item);
            previous = Some(item);
        }
    }

    /// Pictures of people in the call that haven't been asked for yet.
    pub fn wanted_images(&mut self) -> Vec<String> {
        let empty = Voice::default();
        let voice = self
            .view
            .as_ref()
            .and_then(|v| v.voice.as_ref())
            .unwrap_or(&empty);
        let mut urls: Vec<String> =
            voice.members.iter().map(|m| m.avatar.clone()).collect();
        if self.page() == Page::Server
            && let Some(view) = &self.view
        {
            urls.extend(server::wanted_images(view));
        }
        if let Some(bot) = &self.bot {
            urls.extend(bot.friends.iter().map(|f| f.person.avatar.clone()));
            if let Some(id) = self.wanted_server_id()
                && let Some(online) = bot.servers.get(id)
            {
                urls.extend(online.idle.iter().map(|p| p.avatar.clone()));
            }
        }
        let mut wanted = Vec::new();
        for url in urls {
            if !url.is_empty() && self.asked.insert(url.clone()) {
                wanted.push(url);
            }
        }
        wanted
    }

    /// Keeps a downloaded picture.
    pub fn add_image(&mut self, url: String, image: Image) {
        if self.images.len() >= MAX_IMAGES {
            self.images.clear();
            self.asked.clear();
        }
        self.images.insert(url, image);
    }

    /// Notes the control under the pointer (`None` when there isn't
    /// one). Returns whether that changed, so the widget needs
    /// repainting.
    pub fn set_hover(&mut self, hover: Option<Action>) -> bool {
        if self.hover == hover {
            return false;
        }
        self.hover = hover;
        true
    }

    /// Whether `action`'s control is under the pointer.
    fn hovered(&self, action: &Action) -> bool {
        self.hover.as_ref() == Some(action)
    }

    /// Handles a click. Returns a command for the session, if needed.
    pub fn click(&mut self, action: &Action) -> Option<Command> {
        match action {
            Action::Tab(page) => {
                let same = Some(*page) == self.page && self.is_open();
                self.page = Some(*page);
                self.open = Some(!same);
            }
            Action::Header => self.open = Some(!self.is_open()),
            Action::OpenNote {
                channel_id,
                message_id,
            } => {
                return Some(Command::Open {
                    channel_id: channel_id.clone(),
                    message_id: message_id.clone(),
                });
            }
            Action::ServerFollow => {
                self.picked_server = None;
                self.picker = false;
            }
            Action::ServerList => {
                if self.picker {
                    self.picker = false;
                } else {
                    self.open_picker();
                }
            }
            Action::PickerStep(step) => {
                let count = self.view.as_ref().map_or(0, |v| v.servers.len());
                let last = server::picker_pages(count).saturating_sub(1);
                self.picker_page = if *step < 0 {
                    self.picker_page.saturating_sub(1)
                } else {
                    self.picker_page.saturating_add(1).min(last)
                };
            }
            Action::PickServer(id) => {
                self.picked_server = Some(id.clone());
                self.picker = false;
            }
            Action::PromoteFriend(id) => {
                self.friend_top.retain(|f| f != id);
                self.friend_top.insert(0, id.clone());
                self.friend_top.truncate(MAX_FRIEND_TOP);
            }
            Action::CallView => self.call_view = self.call_view.other(),
        }
        if self.is_open() && self.page() == Page::Inbox {
            self.mark_inbox_seen();
        }
        None
    }

    // -- state ----------------------------------------------------------

    /// The page to show.
    fn page(&self) -> Page {
        self.page.unwrap_or(Page::Voice)
    }

    /// Whether the widget is expanded (else it shows the summary line).
    fn is_open(&self) -> bool {
        self.open.unwrap_or(self.was_in_call)
    }

    /// The pages, in tab order (FRIENDS only with a bot).
    fn pages(&self) -> Vec<Page> {
        let mut pages = vec![Page::Voice, Page::Server];
        if self.bot_configured {
            pages.push(Page::Friends);
        }
        pages.push(Page::Inbox);
        pages
    }

    /// SERVER: the server to show (picked, or your call's).
    fn wanted_server_id(&self) -> Option<&str> {
        self.picked_server.as_deref().or_else(|| {
            self.view
                .as_ref()
                .and_then(|v| v.voice.as_ref())
                .and_then(|v| v.guild_id.as_deref())
        })
    }

    /// A `WantServer` command when the server the SERVER page needs
    /// changed since the last one sent.
    pub fn server_request(&mut self) -> Option<Command> {
        let showing =
            self.is_open() && self.page() == Page::Server && !self.picker;
        let wanted = showing
            .then(|| self.wanted_server_id().map(str::to_owned))
            .flatten();
        if self.server_sent && self.sent_server == wanted {
            return None;
        }
        self.server_sent = true;
        self.sent_server.clone_from(&wanted);
        Some(Command::WantServer(wanted))
    }

    /// Opens the server list on the page holding the current server.
    fn open_picker(&mut self) {
        let servers = self
            .view
            .as_ref()
            .map(|v| v.servers.as_slice())
            .unwrap_or_default();
        let current = self.wanted_server_id();
        let index = servers.iter().position(|g| Some(g.id.as_str()) == current);
        self.picker_page = index.map_or(0, |i| i / server::PICKER_ROWS);
        self.picker = true;
    }

    /// Notifications newer than the last one seen.
    fn unread(&self) -> usize {
        self.view.as_ref().map_or(0, |v| {
            v.notifications
                .iter()
                .filter(|n| n.seq > self.inbox_seen)
                .count()
        })
    }

    /// Marks every notification as seen.
    fn mark_inbox_seen(&mut self) {
        if let Some(newest) =
            self.view.as_ref().and_then(|v| v.notifications.front())
        {
            self.inbox_seen = newest.seq;
        }
    }

    /// Notes who started talking since the last update.
    fn track_speaking(&mut self, voice: Option<&Voice>) -> Vec<String> {
        let speaking: HashSet<String> = voice
            .into_iter()
            .flat_map(|v| &v.members)
            .filter(|m| m.speaking)
            .map(|m| m.id.clone())
            .collect();
        let started: Vec<String> = speaking
            .iter()
            .filter(|id| !self.was_speaking.contains(*id))
            .cloned()
            .collect();
        for id in &started {
            self.last_spoke.insert(id.clone(), self.updates);
        }
        self.was_speaking = speaking;
        started
    }

    /// Who has a name row. Up to [`MAX_NAMED`] people everyone does, in
    /// join order. Above that the rows stay put, except: deafened people
    /// drop to the overflow row, and someone in the overflow who starts
    /// talking moves to the top, pushing out whoever talked least
    /// recently (the lowest row on a tie).
    fn sync_named(&mut self, members: &[Member], started: &[String]) {
        if members.len() <= MAX_NAMED {
            self.named = members.iter().map(|m| m.id.clone()).collect();
            return;
        }
        let deaf = |id: &str| members.iter().any(|m| m.id == id && m.deaf);
        let present = |id: &str| members.iter().any(|m| m.id == id);
        // A fresh list starts in join order: nobody has started talking
        // yet as far as the rows are concerned.
        let promote: Vec<&String> = if self.named.is_empty() {
            Vec::new()
        } else {
            started.iter().collect()
        };
        self.named.retain(|id| present(id) && !deaf(id));
        for id in &promote {
            if !self.named.contains(id) && !deaf(id) {
                self.named.insert(0, (*id).clone());
            }
        }
        while self.named.len() > MAX_NAMED {
            let drop = self.quietest_row(&promote);
            if drop < self.named.len() {
                self.named.remove(drop);
            } else {
                self.named.truncate(MAX_NAMED);
            }
        }
        for member in avatar_order(members) {
            if self.named.len() >= MAX_NAMED {
                break;
            }
            if !self.named.contains(&member.id) {
                self.named.push(member.id.clone());
            }
        }
    }

    /// The row whose person talked least recently (lowest on a tie),
    /// skipping anyone just promoted.
    fn quietest_row(&self, promoted: &[&String]) -> usize {
        let spoke = |id: &String| self.last_spoke.get(id).copied().unwrap_or(0);
        self.named
            .iter()
            .enumerate()
            .rev()
            .filter(|(_, id)| !promoted.contains(id))
            .min_by_key(|(_, id)| spoke(id))
            .map_or(self.named.len(), |(index, _)| index)
    }

    // -- layout ---------------------------------------------------------

    /// The rows to draw, top to bottom.
    fn items(&self) -> Vec<Item<'_>> {
        let Some(view) = &self.view else {
            return Vec::new();
        };
        if view.status != Status::Ready {
            return vec![Item::Offline];
        }
        let mut items = vec![Item::Tabs];
        if !self.is_open() {
            items.push(Item::Summary);
            return items;
        }
        match self.page() {
            Page::Voice => {
                let voice = view.voice.as_ref();
                match self.call_view {
                    CallView::Names => {
                        voice_items(voice, &self.named, &mut items);
                    }
                    CallView::Avatars => avatar_items(voice, &mut items),
                }
            }
            Page::Server => server::items(self, view, &mut items),
            Page::Friends => friends::items(self, &mut items),
            Page::Inbox => {
                let notes: Vec<_> =
                    view.notifications.iter().take(INBOX_ROWS).collect();
                if notes.is_empty() {
                    items.push(Item::Dim("NO NOTIFICATIONS"));
                }
                items.extend(notes.into_iter().map(Item::Note));
            }
        }
        items
    }

    /// Total height of `items`.
    fn height(items: &[Item<'_>]) -> f64 {
        let mut total = 0.0;
        let mut previous = None;
        for item in items {
            total += gap_between(previous, item) + Self::item_height(item);
            previous = Some(item);
        }
        total
    }

    /// One row's height.
    fn item_height(item: &Item<'_>) -> f64 {
        match item {
            Item::Offline => OFFLINE_ICON,
            Item::Tabs => TABS_H,
            Item::Dim(_) | Item::Summary | Item::CallHead(_) => LINE,
            Item::Named(_) => FACE_SMALL,
            Item::Rule => 1.0,
            Item::Avatars(people, small) => {
                avatar_rows_height(people.len(), *small, WIDTH)
                    + RING_ROOM * 2.0
            }
            Item::Note(_) => LINE * 2.0 + NOTE_LINE_GAP,
            Item::Server(row) => server::height(row),
            Item::Friends(row) => friends::height(row),
        }
    }

    // -- drawing --------------------------------------------------------

    /// Draws one row with its top at `y`.
    fn draw_item(&self, pen: &mut Pen<'_, '_>, item: &Item<'_>, y: f64) {
        match item {
            Item::Offline => draw_offline(pen),
            Item::Tabs => self.draw_tabs(pen, y),
            Item::Dim(text) => {
                pen.text((0.0, y + BASELINE), TEXT, QUIET, Align::Left, text);
            }
            Item::Summary => self.draw_summary(pen, y),
            Item::CallHead(voice) => {
                let hover = self.hovered(&Action::CallView);
                draw_call_head(pen, voice, (self.call_view.other(), hover), y);
            }
            Item::Named(member) => {
                let centre = (FACE_SMALL / 2.0, y + FACE_SMALL / 2.0);
                let picture = self.images.get(&member.avatar);
                draw_face(pen, member, centre, FACE_SMALL, picture);
                let x = FACE_SMALL + GAP;
                let name = fit(&member.name.to_uppercase(), WIDTH - x, TEXT);
                pen.text(
                    (x, y + FACE_SMALL / 2.0 + TEXT * 0.35),
                    TEXT,
                    1.0,
                    Align::Left,
                    &name,
                );
            }
            Item::Rule => pen.fill(
                Area {
                    x0: 0.0,
                    y0: y,
                    x1: WIDTH,
                    y1: y + 1.0,
                },
                Paint {
                    color: INK,
                    alpha: TRACK,
                },
            ),
            Item::Avatars(people, small) => {
                let top = (0.0, y + RING_ROOM);
                draw_avatar_rows(pen, people, *small, top, WIDTH, &self.images);
            }
            Item::Server(row) => server::draw(self, pen, row, y),
            Item::Friends(row) => friends::draw(self, pen, row, y),
            Item::Note(note) => {
                let action = note.channel_id.as_ref().map(|channel_id| {
                    Action::OpenNote {
                        channel_id: channel_id.clone(),
                        message_id: note.message_id.clone(),
                    }
                });
                let hover = action.as_ref().is_some_and(|a| self.hovered(a));
                draw_note(pen, note, hover, y);
                if let Some(channel_id) = &note.channel_id {
                    let area = Area {
                        x0: 0.0,
                        y0: y,
                        x1: WIDTH,
                        y1: y + Self::item_height(item),
                    };
                    let action = Action::OpenNote {
                        channel_id: channel_id.clone(),
                        message_id: note.message_id.clone(),
                    };
                    pen.hit(area, action);
                }
            }
        }
    }

    /// The tab row: dim labels, the open page underlined, the unread
    /// count over INBOX.
    fn draw_tabs(&self, pen: &mut Pen<'_, '_>, y: f64) {
        let open = self.is_open();
        let unread = self.unread();
        let row = Area {
            x0: 0.0,
            y0: y,
            x1: WIDTH,
            y1: y + TABS_H,
        };
        pen.hit(row, Action::Header);
        let mut x = 0.0;
        for page in self.pages() {
            let label = page.label();
            let on = open && page == self.page();
            let alpha = if on { 1.0 } else { QUIET };
            pen.text((x, y + BASELINE), TEXT, alpha, Align::Left, label);
            let width = text_width(label, TEXT);
            if on {
                pen.fill(
                    Area {
                        x0: x,
                        y0: y + TABS_H - 1.0,
                        x1: x + width,
                        y1: y + TABS_H,
                    },
                    Paint {
                        color: INK,
                        alpha: 1.0,
                    },
                );
            }
            pen.hit(
                Area {
                    x0: x - TAB_GAP / 2.0,
                    x1: x + width + TAB_GAP / 2.0,
                    ..row
                },
                Action::Tab(page),
            );
            x += width;
            if page == Page::Inbox && unread > 0 {
                let count = unread.to_string();
                pen.text_in(
                    ORANGE,
                    (x + 2.0, y + BASELINE - 4.0),
                    8.0,
                    1.0,
                    Align::Left,
                    &count,
                );
                x += 2.0 + text_width(&count, 8.0);
            }
            x += TAB_GAP;
        }
    }

    /// The collapsed line: your call (or "NOT IN VOICE") and unread
    /// notifications.
    fn draw_summary(&self, pen: &mut Pen<'_, '_>, y: f64) {
        let baseline = y + BASELINE;
        let mut x = 0.0;
        if let Some(voice) = self.view.as_ref().and_then(|v| v.voice.as_ref()) {
            pen.circle((x + 2.75, y + LINE / 2.0), 2.75, BLUE, 1.0);
            x += 5.5 + GAP;
            let line = format!(
                "{} · {}",
                voice.channel.to_uppercase(),
                voice.members.len()
            );
            let line = fit(&line, WIDTH - x - 30.0, TEXT);
            pen.text((x, baseline), TEXT, 1.0, Align::Left, &line);
            x += text_width(&line, TEXT);
        } else {
            let text = "NOT IN VOICE";
            pen.text((x, baseline), TEXT, QUIET, Align::Left, text);
            x += text_width(text, TEXT);
        }
        let unread = self.unread();
        if unread > 0 {
            x += GAP;
            let count = unread.to_string();
            let width = text_width(&count, 8.5) + 7.0;
            pen.fill(
                Area {
                    x0: x,
                    y0: y + 1.0,
                    x1: x + width,
                    y1: y + LINE - 1.0,
                },
                Paint {
                    color: ORANGE,
                    alpha: 1.0,
                },
            );
            pen.text_in(
                BLACK,
                (x + width / 2.0, baseline - 0.5),
                8.5,
                1.0,
                Align::Center,
                &count,
            );
        }
    }
}

/// The VOICE page's rows: the channel line, name rows, then anyone else
/// as small pictures.
fn voice_items<'a>(
    voice: Option<&'a Voice>,
    named: &[String],
    items: &mut Vec<Item<'a>>,
) {
    let Some(voice) = voice else {
        items.push(Item::Dim("NOT IN VOICE"));
        return;
    };
    items.push(Item::CallHead(voice));
    for id in named {
        if let Some(member) = voice.members.iter().find(|m| &m.id == id) {
            items.push(Item::Named(member));
        }
    }
    let rest: Vec<&Member> = avatar_order(&voice.members)
        .into_iter()
        .filter(|m| !named.contains(&m.id))
        .collect();
    if !rest.is_empty() {
        items.push(Item::Rule);
        items.push(Item::Avatars(rest, true));
    }
}

/// The VOICE page's rows in the avatars view: the channel line and a
/// row of pictures.
fn avatar_items<'a>(voice: Option<&'a Voice>, items: &mut Vec<Item<'a>>) {
    let Some(voice) = voice else {
        items.push(Item::Dim("NOT IN VOICE"));
        return;
    };
    items.push(Item::CallHead(voice));
    items.push(Item::Avatars(avatar_order(&voice.members), false));
}

/// Join order, deafened last.
fn avatar_order(members: &[Member]) -> Vec<&Member> {
    let mut ordered: Vec<&Member> = members.iter().collect();
    ordered.sort_by_key(|m| (m.deaf, m.join));
    ordered
}

/// The gap between two rows: none above the first, a bigger one under
/// the tabs.
fn gap_between(previous: Option<&Item<'_>>, _item: &Item<'_>) -> f64 {
    match previous {
        None => 0.0,
        Some(Item::Tabs) => TABS_GAP,
        Some(_) => GAP,
    }
}

/// The call's line: channel name in blue, server name dim, then the
/// icon that switches to the `next` view.
fn draw_call_head(
    pen: &mut Pen<'_, '_>,
    voice: &Voice,
    (next, hover): (CallView, bool),
    y: f64,
) {
    let baseline = y + BASELINE;
    let icon_left = WIDTH - VIEW_ICON;
    let alpha = if hover { 1.0 } else { QUIET };
    let spot = (icon_left, y + (LINE - VIEW_ICON) / 2.0);
    draw_view_icon(pen, next, spot, alpha);
    pen.hit(
        Area {
            x0: icon_left - GAP / 2.0,
            y0: y - GAP / 2.0,
            x1: WIDTH + BLEED,
            y1: y + LINE + GAP / 2.0,
        },
        Action::CallView,
    );
    let guild_right = icon_left - GAP;
    let guild = fit(&voice.guild.to_uppercase(), WIDTH * 0.4, TEXT);
    pen.text((guild_right, baseline), TEXT, QUIET, Align::Right, &guild);
    let room = guild_right - text_width(&guild, TEXT) - GAP;
    let channel = fit(&voice.channel.to_uppercase(), room, TEXT);
    pen.text_in(BLUE, (0.0, baseline), TEXT, 1.0, Align::Left, &channel);
}

/// A notification: time and title, then the message. Hovered, the
/// title turns blue and the message brightens.
fn draw_note(pen: &mut Pen<'_, '_>, note: &Notification, hover: bool, y: f64) {
    let baseline = y + BASELINE;
    pen.text((0.0, baseline), TEXT, QUIET, Align::Left, &note.clock);
    let x = text_width("00:00", TEXT) + GAP;
    let title = fit(&note.title.to_uppercase(), WIDTH - x, TEXT);
    let color = if hover { BLUE } else { INK };
    pen.text_in(color, (x, baseline), TEXT, 1.0, Align::Left, &title);
    let body = fit(&note.body.to_uppercase(), WIDTH, TEXT);
    let second = y + LINE + NOTE_LINE_GAP + BASELINE;
    let alpha = if hover { NOTE_HOVER } else { QUIET };
    pen.text((0.0, second), TEXT, alpha, Align::Left, &body);
}

/// The offline icon: a grey ring crossed out.
fn draw_offline(pen: &mut Pen<'_, '_>) {
    let r = OFFLINE_ICON / 2.0;
    let centre = (r, r);
    pen.ring(centre, r - 2.0, 2.0, INK, QUIET);
    let d = (r - 2.0) * std::f64::consts::FRAC_1_SQRT_2;
    let line = [(centre.0 - d, centre.1 + d), (centre.0 + d, centre.1 - d)];
    pen.line_width(
        &line,
        2.0,
        Paint {
            color: INK,
            alpha: QUIET,
        },
    );
}

/// Height of the avatar row(s) for `count` people in `width`.
fn avatar_rows_height(count: usize, small: bool, width: f64) -> f64 {
    let (lines, _, size) = avatar_grid(count, small, width);
    let gap = if small { AVATAR_GAP.1 } else { AVATAR_GAP.0 };
    lines * size + (lines - 1.0).max(0.0) * gap
}

/// Lines, people per line and picture size for an avatar row: lines are
/// balanced (12 people make two lines of 6) and pictures shrink so a
/// line fits `width`.
fn avatar_grid(count: usize, small: bool, width: f64) -> (f64, usize, f64) {
    let lines = count.div_ceil(AVATARS_PER_LINE).max(1);
    let per = count.div_ceil(lines).max(1);
    let (max, gap) = if small {
        (FACE_SMALL, AVATAR_GAP.1)
    } else {
        (FACE, AVATAR_GAP.0)
    };
    let per_f = count_f64(per);
    let fit = (width - (per_f - 1.0) * gap) / per_f;
    (count_f64(lines), per, max.min(fit))
}

/// Draws an avatar row's pictures from the top at `y`.
fn draw_avatar_rows(
    pen: &mut Pen<'_, '_>,
    people: &[&Member],
    small: bool,
    (x, y): (f64, f64),
    width: f64,
    images: &HashMap<String, Image>,
) {
    let (_, per, size) = avatar_grid(people.len(), small, width);
    let gap = if small { AVATAR_GAP.1 } else { AVATAR_GAP.0 };
    for (line, chunk) in (0_u32..).zip(people.chunks(per)) {
        let top = y + f64::from(line) * (size + gap);
        for (column, member) in (0_u32..).zip(chunk) {
            let left = x + f64::from(column) * (size + gap);
            let centre = (left + size / 2.0, top + size / 2.0);
            draw_face(pen, member, centre, size, images.get(&member.avatar));
        }
    }
}

/// A profile picture: a coloured circle with the initial, a ring for
/// speaking / muted / deafened, and a crossed-out badge when muted or
/// deafened.
fn draw_face(
    pen: &mut Pen<'_, '_>,
    member: &Member,
    centre: (f64, f64),
    size: f64,
    picture: Option<&Image>,
) {
    let r = size / 2.0;
    let small = size < FACE * 0.8;
    if let Some(picture) = picture {
        pen.image_circle(centre, r, picture);
    } else {
        let color = FACE_COLORS
            .get(usize::try_from(member.join % 5).unwrap_or(0))
            .copied()
            .unwrap_or(BLUE);
        pen.circle(centre, r, color, 1.0);
        let initial: String = member
            .name
            .chars()
            .next()
            .unwrap_or('?')
            .to_uppercase()
            .collect();
        let text_size = size * if small { 0.42 } else { 0.38 };
        let baseline = (centre.0, centre.1 + text_size * 0.36);
        pen.text_in(BLACK, baseline, text_size, 1.0, Align::Center, &initial);
    }

    let ring = if member.deaf {
        Some(DEAFENED)
    } else if member.mute {
        Some(MUTED)
    } else if member.speaking {
        Some(SPEAKING)
    } else {
        None
    };
    if let Some(ring) = ring {
        let (width, offset) = if small { (1.5, 1.5) } else { (2.0, 2.5) };
        pen.ring(centre, r + offset + width / 2.0, width, ring, 1.0);
    }
    if member.deaf {
        let background = pen.background;
        pen.circle(centre, r, background, DEAFENED_DIM);
    }
    if member.deaf || member.mute {
        draw_state_badge(pen, member.deaf, centre, r, small);
    }
    draw_badges(pen, member, centre, r);
}

/// The muted or deafened badge, bottom-left: the ring's colour with a
/// crossed-out mic or headphones in a darker shade (the wallpaper's
/// icons, on a 16 x 16 grid).
fn draw_state_badge(
    pen: &mut Pen<'_, '_>,
    deaf: bool,
    centre: (f64, f64),
    r: f64,
    small: bool,
) {
    let size = if small { STATE_BADGE.1 } else { STATE_BADGE.0 };
    let half = size / 2.0;
    let badge = (
        centre.0 - r - STATE_BADGE_OUT + half,
        centre.1 + r + STATE_BADGE_OUT - half,
    );
    let (fill, ink) = if deaf {
        (DEAFENED, DEAFENED_DARK)
    } else {
        (MUTED, MUTED_DARK)
    };
    pen.circle(badge, half, fill, 1.0);
    // The symbol fills 80% of the badge.
    let k = size * 0.8 / ICON_GRID;
    let at = |(u, v): (f64, f64)| {
        (
            badge.0 + (u - ICON_GRID / 2.0) * k,
            badge.1 + (v - ICON_GRID / 2.0) * k,
        )
    };
    let paint = Paint {
        color: ink,
        alpha: 1.0,
    };
    let width = 1.8 * k;
    let strokes = if deaf {
        headphones_strokes()
    } else {
        mic_strokes()
    };
    for stroke in strokes {
        let points: Vec<(f64, f64)> = stroke.into_iter().map(at).collect();
        pen.line_width(&points, width, paint);
    }
}

/// A crossed-out microphone on the 16 x 16 grid.
fn mic_strokes() -> Vec<Vec<(f64, f64)>> {
    use std::f64::consts::PI;
    // The capsule: the top half-circle, down the right, the bottom
    // half-circle, back up.
    let mut capsule = half_turn((8.0, 4.0), 2.5, PI, 2.0 * PI);
    capsule.extend(half_turn((8.0, 7.0), 2.5, 0.0, PI));
    if let Some(&first) = capsule.first() {
        capsule.push(first);
    }
    vec![
        capsule,
        half_turn((8.0, 7.5), 5.0, PI, 0.0),
        vec![(8.0, 12.5), (8.0, 14.5)],
        vec![(2.0, 2.0), (14.0, 14.0)],
    ]
}

/// Crossed-out headphones on the 16 x 16 grid.
fn headphones_strokes() -> Vec<Vec<(f64, f64)>> {
    use std::f64::consts::PI;
    let mut band = vec![(2.5, 10.0)];
    band.extend(half_turn((8.0, 8.0), 5.5, PI, 2.0 * PI));
    band.push((13.5, 10.0));
    let cup = |x: f64| {
        vec![
            (x, 9.5),
            (x + 3.0, 9.5),
            (x + 3.0, 14.5),
            (x, 14.5),
            (x, 9.5),
        ]
    };
    vec![band, cup(1.5), cup(11.5), vec![(2.0, 2.0), (14.0, 14.0)]]
}

/// Points along a circle from angle `from` to `to` (radians, clockwise
/// from 3 o'clock, as y points down).
fn half_turn(
    (cx, cy): (f64, f64),
    radius: f64,
    from: f64,
    to: f64,
) -> Vec<(f64, f64)> {
    (0..=ICON_ARC_STEPS)
        .map(|step| {
            let t = from
                + (to - from) * f64::from(step) / f64::from(ICON_ARC_STEPS);
            (cx + radius * t.cos(), cy + radius * t.sin())
        })
        .collect()
}

/// The call view toggle: the view a click switches to, as the wallpaper
/// drew it (three pictures, or two pictures with lines).
fn draw_view_icon(
    pen: &mut Pen<'_, '_>,
    next: CallView,
    (x, y): (f64, f64),
    alpha: f64,
) {
    let k = VIEW_ICON / ICON_GRID;
    let at = |(u, v): (f64, f64)| (x + u * k, y + v * k);
    let paint = Paint { color: INK, alpha };
    let width = 1.3 * k;
    let circle = |pen: &mut Pen<'_, '_>, c: (f64, f64), radius: f64| {
        pen.ring(at(c), radius * k, width, INK, alpha);
    };
    match next {
        CallView::Avatars => {
            for cx in [3.0, 8.0, 13.0] {
                circle(pen, (cx, 8.0), 2.2);
            }
        }
        CallView::Names => {
            for cy in [4.0, 12.0] {
                circle(pen, (3.0, cy), 1.6);
                pen.line_width(&[at((6.5, cy)), at((14.5, cy))], width, paint);
            }
        }
    }
}

/// The status dot (bottom-right), camera badge (top-right) and
/// screen-share dot (top-left).
fn draw_badges(
    pen: &mut Pen<'_, '_>,
    member: &Member,
    centre: (f64, f64),
    r: f64,
) {
    let status = match member.status.as_str() {
        "online" => Some(SPEAKING),
        "idle" => Some(IDLE),
        "dnd" => Some(DND),
        "streaming" => Some(STREAMING),
        _ => None,
    };
    if let Some(color) = status {
        let dot = (centre.0 + r * 0.78, centre.1 + r * 0.78);
        pen.circle(dot, 4.5, BLACK, 1.0);
        pen.circle(dot, 3.0, color, 1.0);
    }
    if member.video {
        let badge = (centre.0 + r * 0.72, centre.1 - r * 0.72);
        pen.circle(badge, r * 0.42, BLACK, 1.0);
        let w = r * 0.28;
        let lens = [
            (badge.0 - w, badge.1 - w * 0.6),
            (badge.0 + w * 0.4, badge.1 - w * 0.6),
            (badge.0 + w * 0.4, badge.1 + w * 0.6),
            (badge.0 - w, badge.1 + w * 0.6),
            (badge.0 - w, badge.1 - w * 0.6),
        ];
        pen.line_width(
            &lens,
            1.0,
            Paint {
                color: BLUE,
                alpha: 1.0,
            },
        );
    }
    if member.live {
        let dot = (centre.0 - r * 0.78, centre.1 - r * 0.78);
        pen.circle(dot, 4.0, BLACK, 1.0);
        pen.circle(dot, 2.75, LIVE, 1.0);
    }
}

/// A bot `Person` as a `Member`, for drawing.
fn member_of(person: &crate::discord::bot::Person) -> Member {
    Member {
        id: person.id.clone(),
        name: person.name.clone(),
        avatar: person.avatar.clone(),
        status: person.status.clone(),
        join: colour_seed(&person.id),
        ..Member::default()
    }
}

/// A stable number from an id, for picking a colour (as the wallpaper
/// hashed ids outside calls).
fn colour_seed(id: &str) -> u64 {
    id.bytes().fold(0_u64, |n, c| {
        n.wrapping_mul(31).wrapping_add(u64::from(c)) % 997
    })
}

/// Approximate width of `text` at `size`.
fn text_width(text: &str, size: f64) -> f64 {
    count_f64(text.chars().count()) * size * CHAR_WIDTH
}

/// `text` cleaned for drawing ([`clean_text`]) and cut to fit `width`
/// at `size`, with an ellipsis if cut.
fn fit(text: &str, width: f64, size: f64) -> String {
    let cleaned = clean_text(text);
    let text = cleaned.as_str();
    let max = (width / (size * CHAR_WIDTH)).floor();
    let max = if max.is_finite() && max >= 1.0 {
        text.chars().count().min(chars_in(max))
    } else {
        0
    };
    if text.chars().count() <= max {
        return text.to_owned();
    }
    let kept: String = text.chars().take(max.saturating_sub(1)).collect();
    format!("{}…", kept.trim_end())
}

/// A whole number of characters from a float count.
#[expect(
    clippy::as_conversions,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "a small non-negative count, checked finite by the caller"
)]
fn chars_in(count: f64) -> usize {
    count.clamp(0.0, 1_000.0) as usize
}

/// A small count as `f64`.
fn count_f64(count: usize) -> f64 {
    u32::try_from(count).map_or(f64::MAX, f64::from)
}

#[cfg(test)]
mod tests {
    use super::{Action, CallView, DiscordUi, Item, MAX_NAMED, Page, fit};
    use crate::discord::model::{
        DiscordView, Member, Notification, Status, Voice,
    };
    use crate::discord::session::Command;

    fn member(id: u64) -> Member {
        Member {
            id: id.to_string(),
            name: format!("user{id}"),
            join: id,
            ..Member::default()
        }
    }

    fn call(members: Vec<Member>) -> DiscordView {
        DiscordView {
            status: Status::Ready,
            voice: Some(Voice {
                channel: "general".into(),
                members,
                ..Voice::default()
            }),
            ..DiscordView::default()
        }
    }

    fn configured() -> DiscordUi {
        let mut ui = DiscordUi::default();
        ui.set_configured(true, false);
        ui
    }

    #[test]
    fn hidden_until_configured() {
        let ui = DiscordUi::default();
        assert_eq!(ui.size(), (0.0, 0.0));
        let ui = configured();
        // Configured but offline: just the icon.
        assert_eq!(ui.size(), (24.0, 24.0));
    }

    #[test]
    fn a_call_opens_voice() {
        let mut ui = configured();
        ui.update(call(vec![member(1), member(2)]));
        assert!(ui.is_open());
        assert_eq!(ui.page(), Page::Voice);
        assert_eq!(ui.named, ["1", "2"]);
        let (_, open) = ui.size();
        ui.update(DiscordView {
            status: Status::Ready,
            ..DiscordView::default()
        });
        assert!(!ui.is_open());
        let (_, closed) = ui.size();
        assert!(open > closed);
    }

    #[test]
    fn talker_in_overflow_moves_up() {
        let mut ui = configured();
        let people: Vec<Member> = (1..=8).map(member).collect();
        ui.update(call(people.clone()));
        assert_eq!(ui.named.len(), MAX_NAMED);
        assert!(!ui.named.contains(&"8".to_owned()));
        // Person 8 starts talking.
        let mut talking = people.clone();
        talking[7].speaking = true;
        ui.update(call(talking));
        assert_eq!(ui.named.first().map(String::as_str), Some("8"));
        assert_eq!(ui.named.len(), MAX_NAMED);
    }

    #[test]
    fn deafened_people_drop_to_overflow() {
        let mut ui = configured();
        let mut people: Vec<Member> = (1..=8).map(member).collect();
        ui.update(call(people.clone()));
        people[0].deaf = true;
        ui.update(call(people));
        assert!(!ui.named.contains(&"1".to_owned()));
    }

    #[test]
    fn counts_unread_notifications() {
        let mut ui = configured();
        let mut view = DiscordView {
            status: Status::Ready,
            ..DiscordView::default()
        };
        view.notifications.push_front(Notification {
            seq: 1,
            ..Notification::default()
        });
        ui.update(view.clone());
        assert_eq!(ui.unread(), 1);
        ui.page = Some(Page::Inbox);
        ui.open = Some(true);
        ui.update(view);
        assert_eq!(ui.unread(), 0);
    }

    #[test]
    fn tabs_open_switch_and_close() {
        use super::Action;
        let mut ui = configured();
        ui.update(DiscordView {
            status: Status::Ready,
            ..DiscordView::default()
        });
        assert!(!ui.is_open());
        assert_eq!(ui.click(&Action::Tab(Page::Inbox)), None);
        assert!(ui.is_open() && ui.page() == Page::Inbox);
        ui.click(&Action::Tab(Page::Voice));
        assert!(ui.is_open() && ui.page() == Page::Voice);
        ui.click(&Action::Tab(Page::Voice));
        assert!(!ui.is_open());
        ui.click(&Action::Header);
        assert!(ui.is_open());
        let open = Action::OpenNote {
            channel_id: "1".into(),
            message_id: None,
        };
        assert!(matches!(ui.click(&open), Some(Command::Open { .. })));
    }

    #[test]
    fn call_view_toggles() {
        let mut ui = configured();
        ui.update(call((1..=3).map(member).collect()));
        assert_eq!(ui.call_view, CallView::Names);
        assert!(ui.items().iter().any(|i| matches!(i, Item::Named(_))));
        ui.click(&Action::CallView);
        assert_eq!(ui.call_view, CallView::Avatars);
        let items = ui.items();
        assert!(!items.iter().any(|i| matches!(i, Item::Named(_))));
        assert!(
            items
                .iter()
                .any(|i| matches!(i, Item::Avatars(people, false)
                    if people.len() == 3))
        );
        ui.click(&Action::CallView);
        assert_eq!(ui.call_view, CallView::Names);
    }

    #[test]
    fn fits_text() {
        assert_eq!(fit("SHORT", 100.0, 10.0), "SHORT");
        // 10.0 * 0.58 per character: 29 fits 5.
        assert_eq!(fit("LONGER NAME", 29.0, 10.0), "LONG…");
    }
}
