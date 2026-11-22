//! What `/event …` (T09) says: search results, the followed events and the
//! follow and unfollow replies.

use chrono::{DateTime, Utc};
use domain::{Event, EventTier};
use store::guilds::FollowedOverview;

use super::{Field, Post, launch_button, pick_count, plural, truncate, ts};

/// Discord allows 25 options per select menu and 25 fields per embed.
pub const MAX_OPTIONS: usize = 25;

/// One search result as `/event search` lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HitRow<'a> {
    pub name: &'a str,
    pub tier: Option<EventTier>,
    pub begin_at: Option<DateTime<Utc>>,
    pub end_at: Option<DateTime<Utc>>,
    pub source: &'a str,
    /// The guild follows the event already.
    pub followed: bool,
}

pub fn tier_label(t: Option<EventTier>) -> String {
    match t {
        None | Some(EventTier::Unknown) => "tier unknown".to_owned(),
        Some(t) => format!("{}-tier", t.as_str().to_uppercase()),
    }
}

/// Dates as Discord markup, shown in each viewer's locale.
fn dates_markup(begin: Option<DateTime<Utc>>, end: Option<DateTime<Utc>>) -> String {
    match (begin, end) {
        (Some(b), Some(e)) => format!("{} – {}", ts(b, 'd'), ts(e, 'd')),
        (Some(b), None) => format!("from {}", ts(b, 'd')),
        (None, Some(e)) => format!("until {}", ts(e, 'd')),
        (None, None) => "dates unknown".to_owned(),
    }
}

/// Dates as plain text (UTC), for select-menu options, which don't render
/// timestamp markup.
fn dates_plain(begin: Option<DateTime<Utc>>, end: Option<DateTime<Utc>>) -> String {
    let d = |t: DateTime<Utc>| t.format("%-d %b %Y").to_string();
    match (begin, end) {
        (Some(b), Some(e)) => format!("{} – {}", d(b), d(e)),
        (Some(b), None) => format!("from {}", d(b)),
        (None, Some(e)) => format!("until {}", d(e)),
        (None, None) => "dates unknown".to_owned(),
    }
}

/// `/event search`: the results (at most [`MAX_OPTIONS`]) and the sources
/// that could not be asked.
pub fn search_results(query: &str, rows: &[HitRow<'_>], skipped: &[(&str, String)]) -> Post {
    let mut lines = Vec::new();
    if rows.is_empty() {
        lines.push("No events found.".to_owned());
    }
    for (n, r) in rows.iter().take(MAX_OPTIONS).enumerate() {
        let followed = if r.followed { " · ✅ followed" } else { "" };
        lines.push(format!(
            "**{}.** {} · {} · {} · `{}`{followed}",
            n + 1,
            r.name,
            tier_label(r.tier),
            dates_markup(r.begin_at, r.end_at),
            r.source
        ));
    }
    if rows.len() > MAX_OPTIONS {
        lines.push(format!(
            "…and {} more. Narrow the search to see them.",
            rows.len() - MAX_OPTIONS
        ));
    }
    for (source, why) in skipped {
        lines.push(format!("⚠️ `{source}` was not searched: {why}"));
    }
    if !rows.is_empty() {
        lines.push(String::new());
        lines.push("Choose an event below to follow it.".to_owned());
    }
    Post {
        title: truncate(&format!("Events matching “{}”", query.trim()), 256),
        description: truncate(&lines.join("\n"), 4000),
        fields: Vec::new(),
        buttons: Vec::new(),
    }
}

/// A search result as a select-menu option: `(label, description)`, each
/// at most 100 characters.
pub fn hit_option(r: &HitRow<'_>) -> (String, String) {
    let followed = if r.followed { " · followed" } else { "" };
    let description = format!(
        "{} · {} · {}{followed}",
        tier_label(r.tier),
        dates_plain(r.begin_at, r.end_at),
        r.source
    );
    (truncate(r.name, 100), truncate(&description, 100))
}

/// The reply after following an event.
pub fn followed(event: &Event, newly: bool) -> Post {
    let (title, description) = if newly {
        (
            format!("Following {}", event.name),
            "Its matches are synced now and show up in the Activity within a minute. \
             Members can pick a match once both teams are known."
                .to_owned(),
        )
    } else {
        (
            format!("Already following {}", event.name),
            "Nothing changed.".to_owned(),
        )
    };
    Post {
        title: truncate(&title, 256),
        description,
        fields: Vec::new(),
        buttons: vec![launch_button()],
    }
}

/// The question before unfollowing an event that has picks.
pub fn unfollow_prompt(name: &str, picks: i64) -> String {
    format!(
        "**{name}** has {} in this server. Unfollowing stops its sync and hides its \
         matches from the lists. Picks, points and history stay as they are. Unfollow it?",
        plural(picks, "pick", "picks")
    )
}

/// The reply after unfollowing.
pub fn unfollowed(name: &str, was_followed: bool) -> String {
    if was_followed {
        format!(
            "Unfollowed **{name}**. Its matches no longer show up in the lists. Picks, \
             points and history stay."
        )
    } else {
        format!("**{name}** was not followed. Nothing changed.")
    }
}

/// `/event list`: the followed events with their counts.
pub fn followed_list(rows: &[FollowedOverview]) -> Post {
    let description = if rows.is_empty() {
        "No events are followed yet. Find one with `/event search`.".to_owned()
    } else if rows.len() > MAX_OPTIONS {
        format!("Showing {MAX_OPTIONS} of {} events.", rows.len())
    } else {
        String::new()
    };
    let fields = rows
        .iter()
        .take(MAX_OPTIONS)
        .map(|r| Field {
            name: truncate(&r.event.name, 256),
            value: format!(
                "{} · {} · {}\n{} · {} · followed by <@{}> {}",
                tier_label(Some(r.event.tier)),
                dates_markup(r.event.begin_at, r.event.end_at),
                r.event.status,
                plural(r.matches, "match", "matches"),
                pick_count(r.picks),
                r.followed_by,
                ts(r.followed_at, 'R')
            ),
        })
        .collect();
    Post {
        title: "Followed events".to_owned(),
        description,
        fields,
        buttons: vec![launch_button()],
    }
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;
    use domain::{EventId, EventStatus};

    use super::*;

    fn t(d: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 11, d, 12, 0, 0).unwrap()
    }

    fn row(name: &str, followed: bool) -> HitRow<'_> {
        HitRow {
            name,
            tier: Some(EventTier::S),
            begin_at: Some(t(20)),
            end_at: Some(t(30)),
            source: "pandascore",
            followed,
        }
    }

    fn all_text(p: &Post) -> String {
        let mut s = format!("{}\n{}", p.title, p.description);
        for f in &p.fields {
            s.push_str(&format!("\n{}\n{}", f.name, f.value));
        }
        s
    }

    #[test]
    fn search_results_show_name_tier_dates_source_and_follow_state() {
        let rows = [row("IEM Cologne", false), row("BLAST Open", true)];
        let p = search_results(
            " iem ",
            &rows,
            &[("other", "hourly budget is short".into())],
        );
        assert_eq!(p.title, "Events matching “iem”");
        let lines: Vec<&str> = p.description.lines().collect();
        let (b, e) = (t(20).timestamp(), t(30).timestamp());
        assert_eq!(
            lines[0],
            format!("**1.** IEM Cologne · S-tier · <t:{b}:d> – <t:{e}:d> · `pandascore`")
        );
        assert!(lines[1].starts_with("**2.** BLAST Open"));
        assert!(lines[1].ends_with("· ✅ followed"));
        assert_eq!(
            lines[2],
            "⚠️ `other` was not searched: hourly budget is short"
        );
        assert_eq!(lines[4], "Choose an event below to follow it.");
    }

    #[test]
    fn search_results_cap_at_25_and_say_when_empty() {
        let names: Vec<String> = (0..30).map(|i| format!("Cup {i}")).collect();
        let rows: Vec<HitRow<'_>> = names.iter().map(|n| row(n, false)).collect();
        let p = search_results("cup", &rows, &[]);
        assert!(p.description.contains("**25.** Cup 24"));
        assert!(!p.description.contains("Cup 25 "));
        assert!(p.description.contains("…and 5 more."));

        let p = search_results("nothing", &[], &[]);
        assert_eq!(p.description, "No events found.");
    }

    #[test]
    fn options_fit_discord_limits_and_use_plain_dates() {
        let long = "x".repeat(150);
        let mut r = row(&long, true);
        let (label, desc) = hit_option(&r);
        assert_eq!(label.chars().count(), 100);
        assert_eq!(
            desc,
            "S-tier · 20 Nov 2026 – 30 Nov 2026 · pandascore · followed"
        );
        r.tier = None;
        r.begin_at = None;
        r.end_at = None;
        r.followed = false;
        assert_eq!(
            hit_option(&r).1,
            "tier unknown · dates unknown · pandascore"
        );
    }

    #[test]
    fn followed_list_and_replies() {
        let event = Event {
            id: EventId(3),
            name: "IEM Cologne".into(),
            tier: EventTier::A,
            begin_at: Some(t(20)),
            end_at: None,
            status: EventStatus::Running,
        };
        let p = followed_list(&[FollowedOverview {
            event: event.clone(),
            followed_by: "200000000000000001".into(),
            followed_at: t(19),
            matches: 1,
            live: 0,
            picks: 0,
        }]);
        assert_eq!(p.fields[0].name, "IEM Cologne");
        assert_eq!(
            p.fields[0].value,
            format!(
                "A-tier · from <t:{}:d> · running\n1 match · no picks yet · followed by \
                 <@200000000000000001> <t:{}:R>",
                t(20).timestamp(),
                t(19).timestamp()
            )
        );
        assert!(followed_list(&[]).description.contains("/event search"));

        assert_eq!(followed(&event, true).title, "Following IEM Cologne");
        assert_eq!(followed(&event, false).description, "Nothing changed.");
        assert!(unfollow_prompt("IEM Cologne", 1).contains("has 1 pick in this server"));
        assert!(unfollowed("IEM Cologne", true).contains("Picks, points and history stay"));

        for text in [
            all_text(&followed_list(&[])),
            all_text(&followed(&event, true)),
            all_text(&search_results("x", &[row("Cup", false)], &[])),
            unfollow_prompt("Cup", 3),
            unfollowed("Cup", true),
        ] {
            assert!(!text.to_lowercase().contains("bet"), "{text}");
        }
    }
}
