//! Pure helpers for the announcer and the admin commands: local time of the
//! digest, result bundling, Discord nonces and score parsing.

use chrono::{DateTime, Duration, NaiveDate, NaiveTime, Utc};
use chrono_tz::Tz;
use domain::MatchId;
// Moved to `domain::localtime` (T14); re-exported for the bot's callers.
pub use domain::localtime::{day_window, local_date, local_to_utc, parse_tz};
use sha2::{Digest, Sha256};
use store::bot::{PostKind, Unannounced};

/// Results that finish within this window of the first one share a post.
pub const BUNDLE_WINDOW: Duration = Duration::minutes(10);
/// At most this many matches go into one result post (embed field limit
/// and readability); the rest follow in the next post.
pub const MAX_BUNDLE: usize = 10;
/// At most this many posts per guild in any hour.
pub const POSTS_PER_HOUR: i64 = 6;
/// How long a *Pick now* click stays valid for the Activity to pick up.
pub const PENDING_VIEW_TTL: Duration = Duration::minutes(10);

/// Parses `HH:MM` (24 h; a single-digit hour is fine) to a time.
pub fn parse_hhmm(s: &str) -> Option<NaiveTime> {
    let (h, m) = s.trim().split_once(':')?;
    if h.is_empty() || h.len() > 2 || m.len() != 2 {
        return None;
    }
    if !h.bytes().chain(m.bytes()).all(|b| b.is_ascii_digit()) {
        return None;
    }
    NaiveTime::from_hms_opt(h.parse().ok()?, m.parse().ok()?, 0)
}

/// The storage form of a digest time: `HH:MM`.
pub fn format_hhmm(t: NaiveTime) -> String {
    t.format("%H:%M").to_string()
}

/// The local date whose digest is due at `now`: today, once the local
/// digest time has passed.
pub fn digest_due(tz: Tz, digest_time: NaiveTime, now: DateTime<Utc>) -> Option<NaiveDate> {
    let today = local_date(tz, now);
    (now >= local_to_utc(tz, today.and_time(digest_time))).then_some(today)
}

/// The next result bundle if it is due: once the earliest unannounced
/// result is [`BUNDLE_WINDOW`] old, it and every result that finished
/// within the window after it (at most [`MAX_BUNDLE`]). `unannounced` must
/// be sorted by finish time.
pub fn due_bundle(unannounced: &[Unannounced], now: DateTime<Utc>) -> Option<Vec<MatchId>> {
    let first = unannounced.first()?;
    let close = first.finished_at + BUNDLE_WINDOW;
    if now < close {
        return None;
    }
    Some(
        unannounced
            .iter()
            .take_while(|u| u.finished_at <= close)
            .take(MAX_BUNDLE)
            .map(|u| u.match_id)
            .collect(),
    )
}

/// The key of a result post: its first match.
pub fn results_key(first: MatchId) -> String {
    format!("m{}", first.0)
}

/// A Discord message nonce for a post (25 characters, the documented
/// maximum): the same post always gets the same nonce, so a resend after
/// a restart returns the message Discord already created.
pub fn nonce(guild_id: &str, kind: PostKind, key: &str) -> String {
    let digest = Sha256::digest(format!("{guild_id}:{}:{key}", kind.as_str()).as_bytes());
    let hex: String = digest.iter().map(|b| format!("{b:02x}")).collect();
    format!("pk{}", &hex[..23])
}

/// A short hash of rendered content, to edit a message only on change.
pub fn content_hash(content: &str) -> String {
    let digest = Sha256::digest(content.as_bytes());
    digest.iter().take(16).map(|b| format!("{b:02x}")).collect()
}

/// Parses a score such as `2-1` or `13:11` (also `–`).
pub fn parse_score<T: std::str::FromStr>(s: &str) -> Option<(T, T)> {
    let s = s.trim();
    let (a, b) = s
        .split_once('-')
        .or_else(|| s.split_once(':'))
        .or_else(|| s.split_once('–'))?;
    Some((a.trim().parse().ok()?, b.trim().parse().ok()?))
}

/// Parses map scores such as `13-11, 9-13, 13-7`, in map order.
pub fn parse_map_scores(s: &str) -> Option<Vec<(u16, u16)>> {
    let maps: Option<Vec<_>> = s
        .split([',', ' ', ';'])
        .filter(|p| !p.trim().is_empty())
        .map(parse_score::<u16>)
        .collect();
    maps.filter(|m| !m.is_empty())
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;

    use super::*;

    fn utc(y: i32, mo: u32, d: u32, h: u32, mi: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(y, mo, d, h, mi, 0).unwrap()
    }

    #[test]
    fn hhmm_parses_and_formats() {
        assert_eq!(parse_hhmm("10:00"), NaiveTime::from_hms_opt(10, 0, 0));
        assert_eq!(parse_hhmm(" 9:05 "), NaiveTime::from_hms_opt(9, 5, 0));
        assert_eq!(parse_hhmm("23:59"), NaiveTime::from_hms_opt(23, 59, 0));
        for bad in [
            "24:00", "10:60", "10", "1000", "10:5", "-1:00", "aa:bb", "123:00",
        ] {
            assert_eq!(parse_hhmm(bad), None, "{bad}");
        }
        assert_eq!(format_hhmm(parse_hhmm("9:05").unwrap()), "09:05");
    }

    #[test]
    fn digest_is_due_at_local_time_across_dst() {
        let berlin = chrono_tz::Europe::Berlin;
        let ten = NaiveTime::from_hms_opt(10, 0, 0).unwrap();
        // Winter (UTC+1): 10:00 local is 09:00 UTC.
        assert_eq!(digest_due(berlin, ten, utc(2026, 11, 22, 8, 59)), None);
        assert_eq!(
            digest_due(berlin, ten, utc(2026, 11, 22, 9, 0)),
            NaiveDate::from_ymd_opt(2026, 11, 22)
        );
        // Summer (UTC+2): 10:00 local is 08:00 UTC.
        assert_eq!(digest_due(berlin, ten, utc(2026, 7, 1, 7, 59)), None);
        assert!(digest_due(berlin, ten, utc(2026, 7, 1, 8, 0)).is_some());
        // The day DST starts (29 Mar 2026): 10:00 is already UTC+2.
        assert_eq!(digest_due(berlin, ten, utc(2026, 3, 29, 7, 59)), None);
        assert!(digest_due(berlin, ten, utc(2026, 3, 29, 8, 0)).is_some());
        // 02:30 does not exist that day: an hour later.
        let gap = NaiveTime::from_hms_opt(2, 30, 0).unwrap();
        assert_eq!(digest_due(berlin, gap, utc(2026, 3, 29, 1, 29)), None);
        assert!(digest_due(berlin, gap, utc(2026, 3, 29, 1, 30)).is_some());
    }

    fn un(id: i64, finished_at: DateTime<Utc>) -> Unannounced {
        Unannounced {
            match_id: MatchId(id),
            finished_at,
        }
    }

    #[test]
    fn bundles_wait_for_the_window_and_group_close_finishes() {
        let t = utc(2026, 11, 22, 18, 0);
        let m = Duration::minutes;
        let list = [un(1, t), un(2, t + m(8)), un(3, t + m(12))];
        assert_eq!(due_bundle(&list, t + m(9)), None);
        assert_eq!(
            due_bundle(&list, t + m(10)),
            Some(vec![MatchId(1), MatchId(2)])
        );
        // 12 minutes after the first: its own bundle, 10 min after it.
        assert_eq!(due_bundle(&list[2..], t + m(21)), None);
        assert_eq!(due_bundle(&list[2..], t + m(22)), Some(vec![MatchId(3)]));
        assert_eq!(due_bundle(&[], t), None);

        let many: Vec<_> = (0..12).map(|i| un(i, t)).collect();
        assert_eq!(due_bundle(&many, t + m(10)).unwrap().len(), MAX_BUNDLE);
    }

    #[test]
    fn nonces_are_stable_short_and_distinct() {
        let a = nonce("1", PostKind::Digest, "2026-11-22");
        assert_eq!(a.len(), 25);
        assert_eq!(a, nonce("1", PostKind::Digest, "2026-11-22"));
        assert_ne!(a, nonce("1", PostKind::Digest, "2026-11-23"));
        assert_ne!(a, nonce("2", PostKind::Digest, "2026-11-22"));
        assert_ne!(a, nonce("1", PostKind::Results, "2026-11-22"));
    }

    #[test]
    fn scores_parse() {
        assert_eq!(parse_score::<u8>("2-1"), Some((2, 1)));
        assert_eq!(parse_score::<u8>(" 2 : 0 "), Some((2, 0)));
        assert_eq!(parse_score::<u8>("1–2"), Some((1, 2)));
        assert_eq!(parse_score::<u8>("2"), None);
        assert_eq!(parse_score::<u8>("a-b"), None);
        assert_eq!(
            parse_map_scores("13-11, 9-13,16-14"),
            Some(vec![(13, 11), (9, 13), (16, 14)])
        );
        assert_eq!(parse_map_scores("13-11 nope"), None);
        assert_eq!(parse_map_scores(" "), None);
    }
}
