//! Local calendar days in a guild's time zone (T08, T14): the bot's daily
//! digest and the leaderboard's match days. Times stay UTC everywhere
//! else; these helpers only find the UTC bounds of a local day.

use chrono::{DateTime, Duration, NaiveDate, NaiveDateTime, NaiveTime, TimeZone, Utc};
use chrono_tz::Tz;

/// Parses an IANA time zone name such as `Europe/Berlin`.
pub fn parse_tz(s: &str) -> Option<Tz> {
    s.trim().parse().ok()
}

/// The UTC instant of a local wall-clock time. In a DST gap (the time does
/// not exist) the time an hour later is used; when the time occurs twice,
/// the first one.
pub fn local_to_utc(tz: Tz, local: NaiveDateTime) -> DateTime<Utc> {
    let pick = |n: NaiveDateTime| tz.from_local_datetime(&n).earliest();
    pick(local)
        .or_else(|| pick(local + Duration::hours(1)))
        .map(|t| t.with_timezone(&Utc))
        // Unreachable for real zones (gaps are at most an hour or two);
        // treat the wall-clock time as UTC rather than fail.
        .unwrap_or_else(|| Utc.from_utc_datetime(&local))
}

/// The local calendar date at `now`.
pub fn local_date(tz: Tz, now: DateTime<Utc>) -> NaiveDate {
    now.with_timezone(&tz).date_naive()
}

/// The UTC bounds `[from, to)` of a local calendar day.
pub fn day_window(tz: Tz, date: NaiveDate) -> (DateTime<Utc>, DateTime<Utc>) {
    let from = local_to_utc(tz, date.and_time(NaiveTime::MIN));
    let next = date.succ_opt().unwrap_or(date);
    (from, local_to_utc(tz, next.and_time(NaiveTime::MIN)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn utc(y: i32, mo: u32, d: u32, h: u32, mi: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(y, mo, d, h, mi, 0).unwrap()
    }

    #[test]
    fn time_zones_parse_by_iana_name() {
        assert_eq!(parse_tz("Europe/Berlin"), Some(chrono_tz::Europe::Berlin));
        assert_eq!(parse_tz("UTC"), Some(chrono_tz::UTC));
        assert_eq!(parse_tz("Mars/Olympus"), None);
        assert_eq!(parse_tz(""), None);
    }

    #[test]
    fn day_window_is_the_local_day() {
        let berlin = chrono_tz::Europe::Berlin;
        let d = NaiveDate::from_ymd_opt(2026, 11, 22).unwrap();
        assert_eq!(
            day_window(berlin, d),
            (utc(2026, 11, 21, 23, 0), utc(2026, 11, 22, 23, 0))
        );
        // A 23-hour day.
        let d = NaiveDate::from_ymd_opt(2026, 3, 29).unwrap();
        assert_eq!(
            day_window(berlin, d),
            (utc(2026, 3, 28, 23, 0), utc(2026, 3, 29, 22, 0))
        );
        // Late evening UTC is already tomorrow in Berlin.
        assert_eq!(
            local_date(berlin, utc(2026, 11, 22, 23, 30)),
            NaiveDate::from_ymd_opt(2026, 11, 23).unwrap()
        );
    }
}
