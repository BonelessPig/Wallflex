//! The FRIENDS page: the friends from the config who are online, with
//! what they're doing; the rest as small pictures that move to the top
//! when clicked. Needs the bot. Redrawn from the user's wallpaper
//! (`dcFriendsHtml`, `dcFriendOrder`).

use super::{
    Action, BLUE, DiscordUi, FACE_SMALL, GAP, INK, Item, LINE, MAX_NAMED,
    QUIET, RING_ROOM, SPEAKING, TEXT, WIDTH, avatar_grid, avatar_rows_height,
    draw_avatar_rows, draw_face, fit, member_of,
};
use crate::canvas::Area;
use crate::discord::bot::{BotStatus, Friend, status_rank};
use crate::discord::model::Member;
use crate::widgets::{Align, Pen};

/// Height of a friend's row (picture, or name over activity).
const ROW_H: f64 = 21.0;
/// Activity text size (0.85vh).
const SUB: f64 = 8.5;
/// Small text size (0.9vh).
const SMALL: f64 = 9.0;
/// Size of the green "online" dot in the counts row.
const DOT: f64 = 5.5;

/// A row of the FRIENDS page.
pub enum Row {
    /// "12 ONLINE · 14 OFFLINE".
    Counts {
        /// Online friends.
        online: usize,
        /// Offline friends.
        offline: usize,
    },
    /// A friend with a row: picture, name, activity.
    Friend {
        /// Picture and name.
        member: Member,
        /// The second line.
        sub: String,
        /// They're in voice (the line is blue).
        in_voice: bool,
    },
    /// Everyone else, as small clickable pictures.
    Overflow(Vec<Member>),
    /// A message (connecting, the bot's error).
    Message(String),
}

/// Adds the FRIENDS page's rows.
pub fn items(ui: &DiscordUi, items: &mut Vec<Item<'_>>) {
    if !ui.bot_configured {
        items.push(Item::Dim("ADD A BOT TOKEN TO SEE FRIENDS"));
        return;
    }
    let Some(bot) = ui.bot.as_ref() else {
        items.push(Item::Dim("CONNECTING THE BOT"));
        return;
    };
    match &bot.status {
        BotStatus::Connecting => {
            items.push(Item::Dim("CONNECTING THE BOT"));
            return;
        }
        BotStatus::Failed(why) => {
            items.push(Item::Friends(Row::Message(why.to_uppercase())));
            return;
        }
        BotStatus::Ready => {}
    }
    let ordered = order(&bot.friends, &ui.friend_top);
    items.push(Item::Friends(Row::Counts {
        online: ordered.len(),
        offline: bot.offline,
    }));
    if ordered.is_empty() {
        items.push(Item::Dim("NOBODY ONLINE"));
        return;
    }
    for friend in ordered.iter().take(MAX_NAMED) {
        let (sub, in_voice) = match &friend.voice {
            Some((channel, _)) => (format!("IN VOICE · {channel}"), true),
            None => (activity_line(friend), false),
        };
        items.push(Item::Friends(Row::Friend {
            member: member_of(&friend.person),
            sub,
            in_voice,
        }));
    }
    let rest: Vec<Member> = ordered
        .iter()
        .skip(MAX_NAMED)
        .map(|f| member_of(&f.person))
        .collect();
    if !rest.is_empty() {
        items.push(Item::Rule);
        items.push(Item::Friends(Row::Overflow(rest)));
    }
}

/// In voice first, then doing something, then by status and name;
/// anyone clicked goes above all of that, most recent click first.
fn order<'a>(friends: &'a [Friend], top: &[String]) -> Vec<&'a Friend> {
    let rank = |f: &Friend| {
        if f.voice.is_some() {
            0
        } else if f.activity.is_some() {
            1
        } else {
            2_u8.saturating_add(status_rank(&f.person.status))
        }
    };
    let mut ordered: Vec<&Friend> = friends.iter().collect();
    ordered.sort_by(|a, b| {
        rank(a).cmp(&rank(b)).then_with(|| {
            a.person
                .name
                .to_lowercase()
                .cmp(&b.person.name.to_lowercase())
        })
    });
    let mut result: Vec<&Friend> = top
        .iter()
        .filter_map(|id| ordered.iter().copied().find(|f| &f.person.id == id))
        .collect();
    result.extend(ordered.into_iter().filter(|f| !top.contains(&f.person.id)));
    result
}

/// What a friend's doing, or their status in words.
fn activity_line(friend: &Friend) -> String {
    friend.activity.clone().unwrap_or_else(|| {
        match friend.person.status.as_str() {
            "streaming" => "STREAMING",
            "idle" => "IDLE",
            "dnd" => "DO NOT DISTURB",
            _ => "ONLINE",
        }
        .to_owned()
    })
}

/// A row's height.
pub fn height(row: &Row) -> f64 {
    match row {
        Row::Counts { .. } | Row::Message(_) => LINE,
        Row::Friend { .. } => ROW_H,
        Row::Overflow(people) => {
            avatar_rows_height(people.len(), true, WIDTH) + RING_ROOM * 2.0
        }
    }
}

/// Draws a row with its top at `y`.
pub fn draw(ui: &DiscordUi, pen: &mut Pen<'_, '_>, row: &Row, y: f64) {
    match row {
        Row::Counts { online, offline } => {
            pen.circle((DOT / 2.0, y + LINE / 2.0), DOT / 2.0, SPEAKING, 1.0);
            let x = DOT + GAP;
            let baseline = y + 10.0;
            let left = format!("{online} ONLINE");
            pen.text((x, baseline), SMALL, 1.0, Align::Left, &left);
            let x = x + super::text_width(&left, SMALL) + GAP;
            let right = format!("· {offline} OFFLINE");
            pen.text((x, baseline), SMALL, QUIET, Align::Left, &right);
        }
        Row::Friend {
            member,
            sub,
            in_voice,
        } => {
            let centre = (FACE_SMALL / 2.0, y + ROW_H / 2.0);
            draw_face(
                pen,
                member,
                centre,
                FACE_SMALL,
                ui.images.get(&member.avatar),
            );
            let x = FACE_SMALL + GAP;
            let name = fit(&member.name.to_uppercase(), WIDTH - x, TEXT);
            pen.text((x, y + 9.0), TEXT, 1.0, Align::Left, &name);
            let sub = fit(&sub.to_uppercase(), WIDTH - x, SUB);
            let (color, alpha) =
                if *in_voice { (BLUE, 1.0) } else { (INK, QUIET) };
            pen.text_in(color, (x, y + 19.5), SUB, alpha, Align::Left, &sub);
        }
        Row::Overflow(people) => {
            let refs: Vec<&Member> = people.iter().collect();
            let top = y + RING_ROOM;
            draw_avatar_rows(pen, &refs, true, (0.0, top), WIDTH, &ui.images);
            let (_, per, size) = avatar_grid(people.len(), true, WIDTH);
            let gap = super::AVATAR_GAP.1;
            for (line, chunk) in (0_u32..).zip(people.chunks(per.max(1))) {
                let row_top = top + f64::from(line) * (size + gap);
                for (column, member) in (0_u32..).zip(chunk) {
                    let left = f64::from(column) * (size + gap);
                    let area = Area {
                        x0: left,
                        y0: row_top,
                        x1: left + size,
                        y1: row_top + size,
                    };
                    pen.hit(area, Action::PromoteFriend(member.id.clone()));
                }
            }
        }
        Row::Message(text) => {
            let text = fit(text, WIDTH, TEXT);
            pen.text((0.0, y + 10.0), TEXT, QUIET, Align::Left, &text);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::order;
    use crate::discord::bot::{Friend, Person};

    fn friend(id: &str, name: &str, status: &str) -> Friend {
        Friend {
            person: Person {
                id: id.into(),
                name: name.into(),
                status: status.into(),
                ..Person::default()
            },
            ..Friend::default()
        }
    }

    #[test]
    fn orders_voice_activity_status_then_clicks() {
        let mut playing = friend("1", "zed", "online");
        playing.activity = Some("Playing x".into());
        let mut talking = friend("2", "amy", "idle");
        talking.voice = Some(("general".into(), "Workbench".into()));
        let idle = friend("3", "bob", "idle");
        let online = friend("4", "cat", "online");
        let all = [playing, talking, idle, online];
        let ids = |list: Vec<&Friend>| {
            list.iter().map(|f| f.person.id.clone()).collect::<Vec<_>>()
        };
        assert_eq!(ids(order(&all, &[])), ["2", "1", "4", "3"]);
        // A click puts bob first.
        assert_eq!(ids(order(&all, &["3".into()])), ["3", "2", "1", "4"]);
    }
}
