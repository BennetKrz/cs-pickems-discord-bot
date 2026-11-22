//! What the bot's posts and replies say. Pure: rows in, a [`Post`] out.
//! `discord.rs` turns a post into a Discord embed with buttons.
//!
//! Times are Discord timestamp markup (`<t:…>`), which every viewer sees
//! in their own time zone and locale, so the backend stays UTC-only. The
//! wording says "pick", never "bet".

use chrono::{DateTime, NaiveDate, Utc};
use domain::{MapStatus, Match, MatchFormat, MatchId, MatchMap, MatchStatus, PickParts, Team};
use serde::Serialize;
use store::bot::TopPicker;
use store::matches::MatchListItem;

pub mod events;

/// Custom ID of the button that opens the Activity.
pub const LAUNCH_ID: &str = "launch";
/// Prefix of the custom ID of a button that opens the Activity on a match.
pub const OPEN_PREFIX: &str = "open:";
/// Discord allows five buttons per row; one row is enough here.
const MAX_BUTTONS: usize = 5;
/// Embed color, close to Discord's blurple.
pub const COLOR: u32 = 0x5865F2;

/// A message: one embed and a row of buttons.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Post {
    pub title: String,
    pub description: String,
    pub fields: Vec<Field>,
    pub buttons: Vec<Button>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Field {
    pub name: String,
    pub value: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Button {
    pub label: String,
    pub custom_id: String,
}

impl Post {
    /// The content hashed to decide whether a sent message needs an edit.
    pub fn fingerprint(&self) -> String {
        serde_json::to_string(self).unwrap_or_default()
    }
}

/// What a button's custom ID asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ButtonAction {
    /// Open the Activity.
    Launch,
    /// Open the Activity on this match.
    Open(MatchId),
}

impl ButtonAction {
    pub fn parse(custom_id: &str) -> Option<Self> {
        if custom_id == LAUNCH_ID {
            return Some(ButtonAction::Launch);
        }
        let id = custom_id.strip_prefix(OPEN_PREFIX)?.parse().ok()?;
        Some(ButtonAction::Open(MatchId(id)))
    }
}

pub(crate) fn launch_button() -> Button {
    Button {
        label: "Open Pick'em".to_owned(),
        custom_id: LAUNCH_ID.to_owned(),
    }
}

fn open_button(verb: &str, item: &MatchListItem) -> Button {
    let label = format!("{verb} {}", versus(item));
    Button {
        label: truncate(&label, 80),
        custom_id: format!("{OPEN_PREFIX}{}", item.m.id.0),
    }
}

/// Match buttons (at most four) followed by the launch button.
fn buttons(verb: &str, items: &[&MatchListItem]) -> Vec<Button> {
    let mut out: Vec<Button> = items
        .iter()
        .take(MAX_BUTTONS - 1)
        .map(|i| open_button(verb, i))
        .collect();
    out.push(launch_button());
    out
}

pub(crate) fn truncate(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        return s.to_owned();
    }
    let mut out: String = s.chars().take(max_chars - 1).collect();
    out.push('…');
    out
}

/// A team as posts show it: its acronym, else its name, else "TBD".
pub fn team_label(t: Option<&Team>) -> String {
    t.map(|t| {
        t.acronym
            .as_deref()
            .filter(|a| !a.trim().is_empty())
            .unwrap_or(&t.name)
            .to_owned()
    })
    .unwrap_or_else(|| "TBD".to_owned())
}

fn versus(i: &MatchListItem) -> String {
    format!(
        "{} vs {}",
        team_label(i.team_a.as_ref()),
        team_label(i.team_b.as_ref())
    )
}

fn format_label(f: MatchFormat) -> &'static str {
    match f {
        MatchFormat::Bo1 => "Bo1",
        MatchFormat::Bo3 => "Bo3",
        MatchFormat::Bo5 => "Bo5",
    }
}

/// Discord timestamp markup: `t` short time, `R` relative, `f` date and time.
pub fn ts(t: DateTime<Utc>, style: char) -> String {
    format!("<t:{}:{style}>", t.timestamp())
}

pub(crate) fn plural(n: i64, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

pub(crate) fn pick_count(n: i64) -> String {
    if n == 0 {
        "no picks yet".to_owned()
    } else {
        plural(n, "pick", "picks")
    }
}

fn points(n: i64) -> String {
    plural(n, "pt", "pts")
}

/// The time picks lock: `lock_at`, else the scheduled start.
pub fn lock_time(m: &Match) -> Option<DateTime<Utc>> {
    m.fields.lock_at.or(m.fields.scheduled_at)
}

/// Whether members can still pick the match at `now`: not locked, not
/// started, both teams known and the lock in the future.
pub fn is_pickable(i: &MatchListItem, now: DateTime<Utc>) -> bool {
    let f = &i.m.fields;
    !f.locked
        && !f.voided
        && matches!(f.status, MatchStatus::Scheduled | MatchStatus::Postponed)
        && i.team_a.is_some()
        && i.team_b.is_some()
        && lock_time(&i.m).is_some_and(|l| l > now)
}

/// The daily digest: the day's matches with lock times and pick counts.
/// Matches that locked since show as closed. `items` are sorted by lock
/// time.
pub fn digest(date: NaiveDate, items: &[MatchListItem], now: DateTime<Utc>) -> Post {
    let mut lines = Vec::new();
    for i in items {
        let head = format!("**{}** · {}", versus(i), format_label(i.m.fields.format));
        let state = if is_pickable(i, now) {
            match lock_time(&i.m) {
                Some(l) => format!("Locks {} ({})", ts(l, 't'), ts(l, 'R')),
                None => "Open".to_owned(),
            }
        } else if i.m.fields.status == MatchStatus::Live {
            "Live · picks closed".to_owned()
        } else if i.m.fields.status == MatchStatus::Finished {
            "Finished · picks closed".to_owned()
        } else if i.team_a.is_none() || i.team_b.is_none() {
            "Teams not known yet".to_owned()
        } else {
            "Picks closed".to_owned()
        };
        lines.push(format!("{head}\n{state} · {}", pick_count(i.pick_count)));
    }
    let open: Vec<&MatchListItem> = items.iter().filter(|i| is_pickable(i, now)).collect();
    let intro = if open.is_empty() {
        "Picks are closed for today's matches.".to_owned()
    } else {
        format!(
            "{} open today. Pick before each match starts.",
            plural(open.len() as i64, "match", "matches")
        )
    };
    Post {
        title: format!("Today's matches · {}", date.format("%a %-d %b")),
        description: truncate(&format!("{intro}\n\n{}", lines.join("\n\n")), 4000),
        fields: Vec::new(),
        buttons: buttons("Pick", &open),
    }
}

/// One finished match of a result post.
#[derive(Debug, Clone)]
pub struct ResultEntry {
    pub item: MatchListItem,
    pub maps: Vec<MatchMap>,
    pub top: Vec<TopPicker>,
}

fn map_line(maps: &[MatchMap]) -> Option<String> {
    let played: Vec<String> = maps
        .iter()
        .filter(|m| m.status == MapStatus::Finished)
        .filter_map(|m| {
            let (a, b) = m.rounds_a.zip(m.rounds_b)?;
            let name = m
                .map_name
                .clone()
                .unwrap_or_else(|| format!("Map {}", m.number));
            Some(format!("{name} {a}–{b}"))
        })
        .collect();
    (!played.is_empty()).then(|| played.join(" · "))
}

fn top_line(top: &[TopPicker]) -> String {
    if top.is_empty() {
        return "Nobody scored on this one.".to_owned();
    }
    const SHOWN: usize = 10;
    let mut names: Vec<String> = top
        .iter()
        .take(SHOWN)
        .map(|p| format!("{} ({})", p.display_name, points(p.points)))
        .collect();
    if top.len() > SHOWN {
        names.push(format!("+{} more", top.len() - SHOWN));
    }
    format!("Best picks: {}", names.join(", "))
}

/// A result post: final score, map scores and the best pickers per match.
pub fn results(entries: &[ResultEntry]) -> Post {
    let fields = entries
        .iter()
        .map(|e| {
            let f = &e.item.m.fields;
            let name = format!(
                "{} {}–{} {}",
                team_label(e.item.team_a.as_ref()),
                f.maps_won_a,
                f.maps_won_b,
                team_label(e.item.team_b.as_ref())
            );
            let mut value = Vec::new();
            if let Some(maps) = map_line(&e.maps) {
                value.push(maps);
            }
            value.push(top_line(&e.top));
            Field {
                name: truncate(&name, 256),
                value: truncate(&value.join("\n"), 1024),
            }
        })
        .collect();
    let items: Vec<&MatchListItem> = entries.iter().map(|e| &e.item).collect();
    Post {
        title: if entries.len() == 1 {
            "Result".to_owned()
        } else {
            "Results".to_owned()
        },
        description: String::new(),
        fields,
        buttons: buttons("View", &items),
    }
}

/// One line of `/leaderboard`, ranked by `store::leaderboard::load` (T14).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LeaderLine {
    pub rank: u32,
    pub name: String,
    pub points: i64,
    /// The part of `points` from live matches.
    pub provisional_points: i64,
}

/// `/leaderboard`: the top members with their ranks (shared ranks repeat)
/// and points; points from live matches are marked.
pub fn leaderboard(rows: &[LeaderLine]) -> Post {
    let description = if rows.is_empty() {
        "No settled picks yet.".to_owned()
    } else {
        rows.iter()
            .map(|r| {
                let live = if r.provisional_points == 0 {
                    String::new()
                } else {
                    format!(" (+{} live)", r.provisional_points)
                };
                format!("**{}.** {} · {}{live}", r.rank, r.name, points(r.points))
            })
            .collect::<Vec<_>>()
            .join("\n")
    };
    Post {
        title: "Leaderboard".to_owned(),
        description,
        fields: Vec::new(),
        buttons: vec![launch_button()],
    }
}

fn parts_line(item: &MatchListItem, parts: &PickParts) -> String {
    let mut out = Vec::new();
    if let Some(w) = parts.winner_team_id {
        let team = [item.team_a.as_ref(), item.team_b.as_ref()]
            .into_iter()
            .flatten()
            .find(|t| t.id == w);
        out.push(format!("Winner {}", team_label(team)));
    }
    if let Some(s) = parts.series {
        out.push(format!("{}–{}", s.a, s.b));
    }
    if !parts.maps.is_empty() {
        out.push(plural(parts.maps.len() as i64, "map pick", "map picks"));
    }
    if out.is_empty() {
        "Empty pick".to_owned()
    } else {
        out.join(" · ")
    }
}

/// `/mypicks`: the member's picks on matches that are still open, and how
/// many open matches they haven't picked.
pub fn my_picks(picks: &[(&MatchListItem, &PickParts)], open_without_pick: usize) -> Post {
    let mut lines: Vec<String> = picks
        .iter()
        .map(|(item, parts)| {
            let lock = lock_time(&item.m)
                .map(|l| format!(" · locks {}", ts(l, 'R')))
                .unwrap_or_default();
            format!("**{}**{lock}\n{}", versus(item), parts_line(item, parts))
        })
        .collect();
    if lines.is_empty() {
        lines.push("You have no open picks.".to_owned());
    }
    if open_without_pick > 0 {
        lines.push(format!(
            "{} without your pick.",
            plural(open_without_pick as i64, "open match", "open matches")
        ));
    }
    Post {
        title: "Your open picks".to_owned(),
        description: truncate(&lines.join("\n\n"), 4000),
        fields: Vec::new(),
        buttons: vec![launch_button()],
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use chrono::TimeZone;
    use domain::{BracketPos, EventId, MatchFields, SeriesScore, TeamId};

    pub fn t(h: u32, m: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 11, 22, h, m, 0).unwrap()
    }

    fn team(id: i64, name: &str, acronym: Option<&str>) -> Team {
        Team {
            id: TeamId(id),
            name: name.to_owned(),
            acronym: acronym.map(str::to_owned),
            logo_url: None,
            country: None,
        }
    }

    pub fn item(id: i64, lock: DateTime<Utc>, status: MatchStatus, picks: i64) -> MatchListItem {
        MatchListItem {
            m: Match {
                id: MatchId(id),
                fields: MatchFields {
                    event_id: EventId(1),
                    stage_id: None,
                    team_a_id: Some(TeamId(1)),
                    team_b_id: Some(TeamId(2)),
                    format: MatchFormat::Bo3,
                    status,
                    scheduled_at: Some(lock),
                    started_at: None,
                    ended_at: None,
                    lock_at: Some(lock),
                    locked: false,
                    voided: false,
                    maps_won_a: 2,
                    maps_won_b: 1,
                    winner_team_id: None,
                    bracket: BracketPos::default(),
                },
            },
            event_name: "IEM".to_owned(),
            stage_name: None,
            team_a: Some(team(1, "Natus Vincere", Some("NAVI"))),
            team_b: Some(team(2, "FaZe Clan", None)),
            pick_count: picks,
            sort_at: Some(lock),
        }
    }

    fn all_text(p: &Post) -> String {
        let mut s = format!("{}\n{}", p.title, p.description);
        for f in &p.fields {
            s.push_str(&format!("\n{}\n{}", f.name, f.value));
        }
        for b in &p.buttons {
            s.push_str(&format!("\n{}", b.label));
        }
        s.to_lowercase()
    }

    #[test]
    fn digest_lists_open_and_closed_matches() {
        let now = t(10, 0);
        let mut closed = item(3, t(9, 0), MatchStatus::Live, 5);
        closed.m.fields.locked = true;
        let items = [
            item(1, t(18, 0), MatchStatus::Scheduled, 0),
            item(2, t(21, 0), MatchStatus::Scheduled, 1),
            closed,
        ];
        let date = NaiveDate::from_ymd_opt(2026, 11, 22).unwrap();
        let p = digest(date, &items, now);
        assert_eq!(p.title, "Today's matches · Sun 22 Nov");
        let lock = t(18, 0).timestamp();
        assert!(
            p.description
                .contains(&format!("Locks <t:{lock}:t> (<t:{lock}:R>)"))
        );
        assert!(p.description.contains("no picks yet"));
        assert!(p.description.contains("1 pick\n") || p.description.contains("· 1 pick"));
        assert!(p.description.contains("Live · picks closed · 5 picks"));
        assert!(p.description.starts_with("2 matches open today"));
        // A button per open match, then the launch button.
        let ids: Vec<&str> = p.buttons.iter().map(|b| b.custom_id.as_str()).collect();
        assert_eq!(ids, ["open:1", "open:2", "launch"]);
        assert_eq!(p.buttons[0].label, "Pick NAVI vs FaZe Clan");
        assert!(!all_text(&p).contains("bet"));
    }

    #[test]
    fn digest_with_more_than_four_open_matches_keeps_five_buttons() {
        let items: Vec<_> = (1..=7)
            .map(|i| item(i, t(12, 0), MatchStatus::Scheduled, 0))
            .collect();
        let p = digest(
            NaiveDate::from_ymd_opt(2026, 11, 22).unwrap(),
            &items,
            t(10, 0),
        );
        assert_eq!(p.buttons.len(), 5);
        assert_eq!(p.buttons[4].custom_id, LAUNCH_ID);
    }

    #[test]
    fn results_show_scores_maps_and_best_pickers() {
        let maps = vec![
            MatchMap {
                match_id: MatchId(1),
                number: 1,
                map_name: Some("Inferno".to_owned()),
                status: MapStatus::Finished,
                rounds_a: Some(13),
                rounds_b: Some(11),
                winner_team_id: Some(TeamId(1)),
            },
            MatchMap {
                match_id: MatchId(1),
                number: 2,
                map_name: None,
                status: MapStatus::Finished,
                rounds_a: Some(9),
                rounds_b: Some(13),
                winner_team_id: Some(TeamId(2)),
            },
            MatchMap {
                match_id: MatchId(1),
                number: 3,
                map_name: None,
                status: MapStatus::NotPlayed,
                rounds_a: None,
                rounds_b: None,
                winner_team_id: None,
            },
        ];
        let top = vec![
            TopPicker {
                user_id: "1".into(),
                display_name: "Alice".into(),
                points: 6,
            },
            TopPicker {
                user_id: "2".into(),
                display_name: "Bob".into(),
                points: 1,
            },
        ];
        let p = results(&[
            ResultEntry {
                item: item(1, t(18, 0), MatchStatus::Finished, 2),
                maps,
                top,
            },
            ResultEntry {
                item: item(2, t(19, 0), MatchStatus::Finished, 0),
                maps: Vec::new(),
                top: Vec::new(),
            },
        ]);
        assert_eq!(p.title, "Results");
        assert_eq!(p.fields[0].name, "NAVI 2–1 FaZe Clan");
        assert_eq!(
            p.fields[0].value,
            "Inferno 13–11 · Map 2 9–13\nBest picks: Alice (6 pts), Bob (1 pt)"
        );
        assert_eq!(p.fields[1].value, "Nobody scored on this one.");
        let ids: Vec<&str> = p.buttons.iter().map(|b| b.custom_id.as_str()).collect();
        assert_eq!(ids, ["open:1", "open:2", "launch"]);
        assert!(!all_text(&p).contains("bet"));
    }

    #[test]
    fn leaderboard_shows_the_given_ranks_and_marks_live_points() {
        let line = |rank, name: &str, points, provisional_points| LeaderLine {
            rank,
            name: name.to_owned(),
            points,
            provisional_points,
        };
        // Ranks come from the tie-break, not from equal points.
        let rows = vec![
            line(1, "Alice", 10, 0),
            line(2, "Bob", 7, 3),
            line(3, "Carol", 7, 0),
            line(3, "Dan", 7, 0),
            line(5, "Eve", 1, 0),
        ];
        let p = leaderboard(&rows);
        assert_eq!(
            p.description,
            "**1.** Alice · 10 pts\n**2.** Bob · 7 pts (+3 live)\n**3.** Carol · 7 pts\n\
             **3.** Dan · 7 pts\n**5.** Eve · 1 pt"
        );
        assert!(!all_text(&p).contains("bet"));
        assert_eq!(p.buttons, vec![launch_button()]);
        assert_eq!(leaderboard(&[]).description, "No settled picks yet.");
    }

    #[test]
    fn my_picks_lists_parts_and_missing_picks() {
        let i = item(1, t(18, 0), MatchStatus::Scheduled, 3);
        let parts = PickParts {
            winner_team_id: Some(TeamId(1)),
            series: Some(SeriesScore { a: 2, b: 1 }),
            maps: Vec::new(),
        };
        let p = my_picks(&[(&i, &parts)], 2);
        let lock = t(18, 0).timestamp();
        assert_eq!(
            p.description,
            format!(
                "**NAVI vs FaZe Clan** · locks <t:{lock}:R>\nWinner NAVI · 2–1\n\n\
                 2 open matches without your pick."
            )
        );
        assert_eq!(my_picks(&[], 0).description, "You have no open picks.");
    }

    #[test]
    fn button_custom_ids_round_trip() {
        assert_eq!(ButtonAction::parse("launch"), Some(ButtonAction::Launch));
        assert_eq!(
            ButtonAction::parse("open:42"),
            Some(ButtonAction::Open(MatchId(42)))
        );
        for bad in ["open:", "open:x", "Launch", "pick:1", ""] {
            assert_eq!(ButtonAction::parse(bad), None, "{bad}");
        }
    }

    #[test]
    fn labels_are_truncated_to_discord_limits() {
        let mut i = item(1, t(18, 0), MatchStatus::Scheduled, 0);
        i.team_a = Some(team(1, &"x".repeat(100), None));
        let b = open_button("Pick", &i);
        assert_eq!(b.label.chars().count(), 80);
        assert!(b.label.ends_with('…'));
    }
}
