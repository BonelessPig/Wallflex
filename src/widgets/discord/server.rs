//! The SERVER page: a server's occupied voice channels, for your call's
//! server or one picked from your server list. Redrawn from the user's
//! wallpaper (`dcServerHtml`, `dcPickerHtml`, `dcChannelsHtml`).

use super::{
    Action, BLACK, BLUE, DiscordUi, FACE_COLORS, GAP, INK, Item, LINE, QUIET,
    RING_ROOM, TEXT, WIDTH, avatar_order, avatar_rows_height, draw_avatar_rows,
    fit, text_width,
};
use crate::canvas::{Area, Paint};
use crate::discord::model::{DiscordView, Guild, Member, VoiceChannel};
use crate::widgets::{Align, Pen};

/// Servers per page of the server list.
pub const PICKER_ROWS: usize = 8;
/// Voice channels listed.
const MAX_CHANNELS: usize = 4;
/// Server icon size (1.7vh) and corner radius (0.5vh).
const ICON: (f64, f64) = (17.0, 5.0);
/// Button size (1.3vh) and the gap between buttons.
const BUTTON: (f64, f64) = (13.0, 7.0);
/// Indent of a channel's pictures (1.6vh).
const INDENT: f64 = 16.0;
/// Speaker icon size (1vh).
const SPEAKER: f64 = 10.0;
/// Small text (0.9vh).
const SMALL: f64 = 9.0;
/// Opacity of a button that can't be used (0.2).
const OFF: f64 = 0.2;
/// Icons are drawn on a 16-unit grid, like the wallpaper's SVGs.
const GRID: f64 = 16.0;

/// A row of the SERVER page.
pub enum Row<'a> {
    /// The server's icon and name, with the buttons.
    Head {
        /// Server id.
        id: &'a str,
        /// Server name.
        name: &'a str,
        /// Icon URL.
        icon: Option<&'a str>,
    },
    /// Following the call, but the call isn't in a server.
    NoServer,
    /// The server list's heading, with the buttons.
    PickerHead,
    /// A server in the list.
    Pick {
        /// The server.
        guild: &'a Guild,
        /// A note on the right ("IN VOICE").
        tag: &'static str,
        /// Whether it's the server being shown.
        current: bool,
    },
    /// Page arrows under the list.
    Pager {
        /// Current page, from 0.
        page: usize,
        /// Number of pages.
        pages: usize,
    },
    /// An occupied voice channel's name.
    Channel(&'a VoiceChannel),
    /// Its people, indented.
    People(Vec<&'a Member>),
    /// A dim note ("+ 4 EMPTY").
    Note(String),
    /// "12 ONLINE · 48 MEMBERS" (from the bot).
    Counts {
        /// Online members.
        online: usize,
        /// All members.
        total: u64,
    },
    /// Online people not in voice (from the bot).
    Idle(Vec<Member>),
    /// The bot isn't in this server.
    NeedBot,
}

/// Adds the SERVER page's rows.
pub fn items<'a>(
    ui: &'a DiscordUi,
    view: &'a DiscordView,
    items: &mut Vec<Item<'a>>,
) {
    if ui.picker {
        picker_items(ui, view, items);
        return;
    }
    let Some(wanted) = ui.wanted_server_id() else {
        items.push(Item::Server(Row::NoServer));
        return;
    };
    let known = view.servers.iter().find(|g| g.id == wanted);
    let server = view.server.as_ref().filter(|s| s.id == wanted);
    let name = known
        .map(|g| g.name.as_str())
        .or_else(|| server.map(|s| s.name.as_str()));
    let icon = known
        .and_then(|g| g.icon.as_deref())
        .or_else(|| server.and_then(|s| s.icon.as_deref()));
    items.push(Item::Server(Row::Head {
        id: known.map_or(wanted, |g| g.id.as_str()),
        name: name.unwrap_or(""),
        icon,
    }));
    let Some(server) = server else {
        items.push(Item::Dim("LOADING"));
        return;
    };
    let online = ui.bot.as_ref().and_then(|b| b.servers.get(wanted));
    if let Some(online) = online {
        items.push(Item::Server(Row::Counts {
            online: online.online,
            total: online.total,
        }));
    }
    items.push(Item::Rule);
    for channel in server.channels.iter().take(MAX_CHANNELS) {
        items.push(Item::Server(Row::Channel(channel)));
        items.push(Item::Server(Row::People(avatar_order(&channel.members))));
    }
    let mut notes = Vec::new();
    if server.channels.is_empty() {
        notes.push("NOBODY IN VOICE".to_owned());
    }
    let more = server.channels.len().saturating_sub(MAX_CHANNELS);
    if more > 0 {
        notes.push(format!("+ {more} MORE IN USE"));
    }
    if server.empty > 0 {
        let plus = if server.channels.is_empty() { "" } else { "+ " };
        notes.push(format!("{plus}{} EMPTY", server.empty));
    }
    if !notes.is_empty() {
        items.push(Item::Server(Row::Note(notes.join(" · "))));
    }
    if ui.bot_configured {
        match online {
            Some(online) if !online.idle.is_empty() => {
                items.push(Item::Rule);
                items.push(Item::Server(Row::Note(
                    "ONLINE, NOT IN VOICE".into(),
                )));
                let people = online.idle.iter().map(super::member_of).collect();
                items.push(Item::Server(Row::Idle(people)));
            }
            Some(_) => {}
            None => {
                items.push(Item::Rule);
                items.push(Item::Server(Row::NeedBot));
            }
        }
    }
}

/// The server list's rows.
fn picker_items<'a>(
    ui: &'a DiscordUi,
    view: &'a DiscordView,
    items: &mut Vec<Item<'a>>,
) {
    items.push(Item::Server(Row::PickerHead));
    if view.servers.is_empty() {
        items.push(Item::Dim("LOADING"));
        return;
    }
    let pages = view.servers.len().div_ceil(PICKER_ROWS).max(1);
    let page = ui.picker_page.min(pages.saturating_sub(1));
    let voice_guild = view.voice.as_ref().and_then(|v| v.guild_id.as_deref());
    let current = ui.wanted_server_id();
    items.push(Item::Rule);
    let first = page.saturating_mul(PICKER_ROWS);
    for guild in view.servers.iter().skip(first).take(PICKER_ROWS) {
        let bot_missing = ui.bot.as_ref().is_some_and(|b| {
            b.status == crate::discord::bot::BotStatus::Ready
                && !b.servers.contains_key(&guild.id)
        });
        let tag = if Some(guild.id.as_str()) == voice_guild {
            "IN VOICE"
        } else if bot_missing {
            "NO BOT"
        } else {
            ""
        };
        items.push(Item::Server(Row::Pick {
            guild,
            tag,
            current: current == Some(guild.id.as_str()),
        }));
    }
    if pages > 1 {
        items.push(Item::Server(Row::Pager { page, pages }));
    }
}

/// A row's height.
pub fn height(row: &Row<'_>) -> f64 {
    match row {
        Row::Head { .. } | Row::Pick { .. } => ICON.0,
        Row::NoServer
        | Row::PickerHead
        | Row::Pager { .. }
        | Row::Channel(_)
        | Row::Note(_)
        | Row::Counts { .. }
        | Row::NeedBot => LINE,
        Row::Idle(people) => {
            avatar_rows_height(people.len(), true, WIDTH) + RING_ROOM * 2.0
        }
        Row::People(people) => {
            avatar_rows_height(people.len(), true, WIDTH - INDENT)
                + RING_ROOM * 2.0
        }
    }
}

/// Draws a row with its top at `y`.
pub fn draw(ui: &DiscordUi, pen: &mut Pen<'_, '_>, row: &Row<'_>, y: f64) {
    match row {
        Row::Head { id, name, icon } => {
            draw_head(ui, pen, (id, name, *icon), y);
        }
        Row::NoServer => draw_dim_head(ui, pen, "NOT IN A SERVER CALL", y),
        Row::PickerHead => draw_dim_head(ui, pen, "PICK A SERVER", y),
        Row::Pick {
            guild,
            tag,
            current,
        } => draw_pick(ui, pen, guild, (tag, *current), y),
        Row::Pager { page, pages } => draw_pager(pen, *page, *pages, y),
        Row::Channel(channel) => draw_channel(pen, channel, y),
        Row::People(people) => {
            let top = (INDENT, y + RING_ROOM);
            draw_avatar_rows(
                pen,
                people,
                true,
                top,
                WIDTH - INDENT,
                &ui.images,
            );
        }
        Row::Note(text) => {
            pen.text((0.0, y + 10.0), SMALL, QUIET, Align::Left, text);
        }
        Row::Counts { online, total } => draw_counts(pen, *online, *total, y),
        Row::Idle(people) => {
            let refs: Vec<&Member> = people.iter().collect();
            let top = (0.0, y + RING_ROOM);
            draw_avatar_rows(pen, &refs, true, top, WIDTH, &ui.images);
        }
        Row::NeedBot => {
            let text = "ADD THE BOT TO THIS SERVER TO SEE WHO'S ONLINE";
            pen.text((0.0, y + 10.0), SMALL, QUIET, Align::Left, text);
        }
    }
}

/// The server's icon and name, with the buttons.
fn draw_head(
    ui: &DiscordUi,
    pen: &mut Pen<'_, '_>,
    (id, name, icon): (&str, &str, Option<&str>),
    y: f64,
) {
    draw_icon(ui, pen, (0.0, y), id, name, icon);
    let x = ICON.0 + GAP;
    let room = WIDTH - x - buttons_width() - GAP;
    let name = fit(&name.to_uppercase(), room, TEXT);
    let baseline = y + ICON.0 / 2.0 + TEXT * 0.35;
    pen.text((x, baseline), TEXT, 1.0, Align::Left, &name);
    draw_buttons(ui, pen, y + ICON.0 / 2.0);
}

/// A dim heading line with the buttons.
fn draw_dim_head(ui: &DiscordUi, pen: &mut Pen<'_, '_>, text: &str, y: f64) {
    pen.text((0.0, y + 10.0), TEXT, QUIET, Align::Left, text);
    draw_buttons(ui, pen, y + LINE / 2.0);
}

/// A server in the list: icon, name (blue if shown), tag.
fn draw_pick(
    ui: &DiscordUi,
    pen: &mut Pen<'_, '_>,
    guild: &Guild,
    (tag, current): (&str, bool),
    y: f64,
) {
    let icon = guild.icon.as_deref();
    draw_icon(ui, pen, (0.0, y), &guild.id, &guild.name, icon);
    let x = ICON.0 + GAP;
    let baseline = y + ICON.0 / 2.0 + TEXT * 0.35;
    pen.text((WIDTH, baseline), SMALL, QUIET, Align::Right, tag);
    let room = WIDTH - x - text_width(tag, SMALL) - GAP;
    let name = fit(&guild.name.to_uppercase(), room, TEXT);
    let color = if current { BLUE } else { INK };
    pen.text_in(color, (x, baseline), TEXT, 1.0, Align::Left, &name);
    let area = Area {
        x0: 0.0,
        y0: y,
        x1: WIDTH,
        y1: y + ICON.0,
    };
    pen.hit(area, Action::PickServer(guild.id.clone()));
}

/// An occupied voice channel: speaker, name, head count.
fn draw_channel(pen: &mut Pen<'_, '_>, channel: &VoiceChannel, y: f64) {
    let color = if channel.you { BLUE } else { INK };
    let alpha = if channel.you { 1.0 } else { QUIET };
    let top = y + (LINE - SPEAKER) / 2.0;
    glyph(pen, (0.0, top), SPEAKER, &speaker(), Paint { color, alpha });
    let x = SPEAKER + GAP;
    let count = channel.members.len().to_string();
    let baseline = y + 10.0;
    pen.text((WIDTH, baseline), TEXT, QUIET, Align::Right, &count);
    let room = WIDTH - x - text_width(&count, TEXT) - GAP;
    let name = fit(&channel.name.to_uppercase(), room, TEXT);
    pen.text_in(color, (x, baseline), TEXT, 1.0, Align::Left, &name);
}

/// "12 ONLINE · 48 MEMBERS" with a green dot.
fn draw_counts(pen: &mut Pen<'_, '_>, online: usize, total: u64, y: f64) {
    let dot = 5.5;
    pen.circle((dot / 2.0, y + LINE / 2.0), dot / 2.0, super::SPEAKING, 1.0);
    let left = format!("{online} ONLINE");
    let x = dot + GAP;
    pen.text((x, y + 10.0), SMALL, 1.0, Align::Left, &left);
    let x = x + text_width(&left, SMALL) + GAP;
    let right = format!("· {total} MEMBERS");
    pen.text((x, y + 10.0), SMALL, QUIET, Align::Left, &right);
}

/// A server's icon (its picture, or its initial on a colour) with its
/// top-left at `(x, y)`.
fn draw_icon(
    ui: &DiscordUi,
    pen: &mut Pen<'_, '_>,
    (x, y): (f64, f64),
    id: &str,
    name: &str,
    icon: Option<&str>,
) {
    let half = ICON.0 / 2.0;
    let centre = (x + half, y + half);
    if let Some(picture) = icon.and_then(|url| ui.images.get(url)) {
        pen.image_rounded(centre, half, ICON.1, picture);
        return;
    }
    let colour = FACE_COLORS.get(colour_index(id)).copied().unwrap_or(BLUE);
    pen.rounded(centre, half, ICON.1, colour, 1.0);
    let initial: String =
        name.chars().next().unwrap_or('?').to_uppercase().collect();
    let size = 8.5;
    pen.text_in(
        BLACK,
        (centre.0, centre.1 + size * 0.36),
        size,
        1.0,
        Align::Center,
        &initial,
    );
}

/// The wallpaper's colour for an id: a hash of its characters.
fn colour_index(id: &str) -> usize {
    let hash = id.bytes().fold(0_u32, |n, c| {
        n.wrapping_mul(31).wrapping_add(u32::from(c)) % 997
    });
    usize::try_from(hash)
        .ok()
        .and_then(|h| h.checked_rem(FACE_COLORS.len()))
        .unwrap_or(0)
}

/// Width of the two buttons and their gap.
fn buttons_width() -> f64 {
    BUTTON.0 * 2.0 + BUTTON.1
}

/// The follow-my-call and server-list buttons, right-aligned, centred on
/// `middle`.
fn draw_buttons(ui: &DiscordUi, pen: &mut Pen<'_, '_>, middle: f64) {
    let top = middle - BUTTON.0 / 2.0;
    let follow_on = ui.picked_server.is_none() && !ui.picker;
    let list_x = WIDTH - BUTTON.0;
    let follow_x = list_x - BUTTON.1 - BUTTON.0;
    button(
        pen,
        (follow_x, top),
        &headphones(),
        follow_on,
        Action::ServerFollow,
    );
    button(pen, (list_x, top), &list(), ui.picker, Action::ServerList);
}

/// One button: an icon, blue and underlined when on, and its hit area.
fn button(
    pen: &mut Pen<'_, '_>,
    (x, y): (f64, f64),
    paths: &[Vec<(f64, f64)>],
    on: bool,
    action: Action,
) {
    let paint = if on {
        Paint {
            color: BLUE,
            alpha: 1.0,
        }
    } else {
        Paint {
            color: INK,
            alpha: QUIET,
        }
    };
    glyph(pen, (x, y), BUTTON.0, paths, paint);
    let area = Area {
        x0: x,
        y0: y,
        x1: x + BUTTON.0,
        y1: y + BUTTON.0 + 3.0,
    };
    if on {
        pen.fill(
            Area {
                y0: area.y1 - 1.0,
                ..area
            },
            paint,
        );
    }
    pen.hit(
        Area {
            x0: x - BUTTON.1 / 2.0,
            x1: x + BUTTON.0 + BUTTON.1 / 2.0,
            ..area
        },
        action,
    );
}

/// The arrows and "PAGE 2 / 7" under the server list.
fn draw_pager(pen: &mut Pen<'_, '_>, page: usize, pages: usize, y: f64) {
    let top = y + (LINE - BUTTON.0) / 2.0;
    let arrow = |pen: &mut Pen<'_, '_>,
                 x: f64,
                 paths: &[Vec<(f64, f64)>],
                 usable: bool,
                 step: i8| {
        let alpha = if usable { QUIET } else { OFF };
        glyph(pen, (x, top), BUTTON.0, paths, Paint { color: INK, alpha });
        if usable {
            let area = Area {
                x0: x - BUTTON.1,
                y0: y,
                x1: x + BUTTON.0 + BUTTON.1,
                y1: y + LINE,
            };
            pen.hit(area, Action::PickerStep(step));
        }
    };
    arrow(pen, 0.0, &chevron(true), page > 0, -1);
    arrow(
        pen,
        WIDTH - BUTTON.0,
        &chevron(false),
        page.saturating_add(1) < pages,
        1,
    );
    let label = format!("PAGE {} / {pages}", page.saturating_add(1));
    pen.text((WIDTH / 2.0, y + 10.0), SMALL, QUIET, Align::Center, &label);
}

/// Strokes `paths` (on the 16-unit icon grid) into a `size` square with
/// its top-left at `(x, y)`.
fn glyph(
    pen: &mut Pen<'_, '_>,
    (x, y): (f64, f64),
    size: f64,
    paths: &[Vec<(f64, f64)>],
    paint: Paint,
) {
    let k = size / GRID;
    for path in paths {
        let points: Vec<(f64, f64)> = path
            .iter()
            .map(|&(px, py)| (x + px * k, y + py * k))
            .collect();
        pen.line_width(&points, 1.3 * k, paint);
    }
}

/// Points around a circle from `from` to `to` radians (0 = right,
/// clockwise on screen).
fn arc_points(
    centre: (f64, f64),
    radius: f64,
    from: f64,
    to: f64,
) -> Vec<(f64, f64)> {
    const STEPS: u32 = 12;
    (0..=STEPS)
        .map(|i| {
            let angle = from + (to - from) * f64::from(i) / f64::from(STEPS);
            (
                centre.0 + radius * angle.cos(),
                centre.1 + radius * angle.sin(),
            )
        })
        .collect()
}

/// A rectangle's outline as a closed path.
fn rect_path(x: f64, y: f64, w: f64, h: f64) -> Vec<(f64, f64)> {
    vec![(x, y), (x + w, y), (x + w, y + h), (x, y + h), (x, y)]
}

/// Headphones: follow the server of my call.
fn headphones() -> Vec<Vec<(f64, f64)>> {
    use std::f64::consts::PI;
    let mut band = vec![(2.5, 10.0)];
    band.extend(arc_points((8.0, 8.0), 5.5, PI, 2.0 * PI));
    band.push((13.5, 10.0));
    vec![
        band,
        rect_path(1.5, 9.5, 3.0, 5.0),
        rect_path(11.5, 9.5, 3.0, 5.0),
    ]
}

/// Two squares and lines: pick a server.
fn list() -> Vec<Vec<(f64, f64)>> {
    vec![
        rect_path(1.5, 2.0, 4.0, 4.0),
        rect_path(1.5, 10.0, 4.0, 4.0),
        vec![(8.0, 4.0), (14.5, 4.0)],
        vec![(8.0, 12.0), (14.5, 12.0)],
    ]
}

/// A page arrow.
fn chevron(back: bool) -> Vec<Vec<(f64, f64)>> {
    if back {
        vec![vec![(10.0, 3.0), (5.0, 8.0), (10.0, 13.0)]]
    } else {
        vec![vec![(6.0, 3.0), (11.0, 8.0), (6.0, 13.0)]]
    }
}

/// A speaker: a voice channel.
fn speaker() -> Vec<Vec<(f64, f64)>> {
    use std::f64::consts::FRAC_PI_4;
    vec![
        vec![
            (2.0, 6.0),
            (5.0, 6.0),
            (9.0, 3.0),
            (9.0, 13.0),
            (5.0, 10.0),
            (2.0, 10.0),
            (2.0, 6.0),
        ],
        arc_points((8.0, 8.0), 3.5, -FRAC_PI_4, FRAC_PI_4),
    ]
}

/// Server icons and channel pictures the page shows.
pub fn wanted_images(view: &DiscordView) -> Vec<String> {
    let icons = view.servers.iter().filter_map(|g| g.icon.clone());
    let server = view.server.iter().flat_map(|s| {
        let people = s
            .channels
            .iter()
            .flat_map(|c| c.members.iter().map(|m| m.avatar.clone()));
        s.icon.clone().into_iter().chain(people)
    });
    icons.chain(server).collect()
}

/// The page count for `count` servers.
pub fn picker_pages(count: usize) -> usize {
    count.div_ceil(PICKER_ROWS).max(1)
}

#[cfg(test)]
mod tests {
    use super::{colour_index, picker_pages};

    #[test]
    fn pages_and_colours() {
        assert_eq!(picker_pages(0), 1);
        assert_eq!(picker_pages(8), 1);
        assert_eq!(picker_pages(9), 2);
        assert!(colour_index("1234") < 5);
        assert_eq!(colour_index("1234"), colour_index("1234"));
    }
}
