//! When the daily backup runs: a local wall-clock time in the zone from
//! `TZ` (UTC by default).

use chrono::{DateTime, Duration, NaiveDate, NaiveDateTime, NaiveTime, TimeZone, Utc};
use chrono_tz::Tz;

/// The UTC instant of a local wall-clock time. In a DST gap (the time does
/// not exist that day) the time an hour later is used; when the time occurs
/// twice, the first one.
fn local_to_utc(tz: Tz, local: NaiveDateTime) -> DateTime<Utc> {
    let pick = |n: NaiveDateTime| tz.from_local_datetime(&n).earliest();
    pick(local)
        .or_else(|| pick(local + Duration::hours(1)))
        .map(|t| t.with_timezone(&Utc))
        // Unreachable for real zones; treat the time as UTC rather than fail.
        .unwrap_or_else(|| Utc.from_utc_datetime(&local))
}

fn slot_on(tz: Tz, date: NaiveDate, at: NaiveTime) -> DateTime<Utc> {
    local_to_utc(tz, date.and_time(at))
}

/// The first daily slot strictly after `now`.
pub fn next_run(now: DateTime<Utc>, at: NaiveTime, tz: Tz) -> DateTime<Utc> {
    let today = now.with_timezone(&tz).date_naive();
    let mut date = today - Duration::days(1);
    loop {
        let slot = slot_on(tz, date, at);
        if slot > now {
            return slot;
        }
        date += Duration::days(1);
    }
}

/// The newest daily slot at or before `now`.
pub fn previous_run(now: DateTime<Utc>, at: NaiveTime, tz: Tz) -> DateTime<Utc> {
    let today = now.with_timezone(&tz).date_naive();
    let mut date = today + Duration::days(1);
    loop {
        let slot = slot_on(tz, date, at);
        if slot <= now {
            return slot;
        }
        date -= Duration::days(1);
    }
}

/// Parses `HH:MM` (24 h) for `BACKUP_TIME`.
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

#[cfg(test)]
mod tests {
    use super::*;
    use chrono_tz::Europe::Berlin;

    fn utc(y: i32, mo: u32, d: u32, h: u32, mi: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(y, mo, d, h, mi, 0).unwrap()
    }

    fn four() -> NaiveTime {
        NaiveTime::from_hms_opt(4, 0, 0).unwrap()
    }

    #[test]
    fn next_and_previous_in_utc() {
        let now = utc(2026, 11, 22, 3, 59);
        assert_eq!(next_run(now, four(), Tz::UTC), utc(2026, 11, 22, 4, 0));
        assert_eq!(previous_run(now, four(), Tz::UTC), utc(2026, 11, 21, 4, 0));
        let now = utc(2026, 11, 22, 4, 0);
        assert_eq!(next_run(now, four(), Tz::UTC), utc(2026, 11, 23, 4, 0));
        assert_eq!(previous_run(now, four(), Tz::UTC), now);
    }

    #[test]
    fn local_time_in_berlin() {
        // Winter: 04:00 CET is 03:00 UTC.
        let now = utc(2026, 11, 22, 2, 0);
        assert_eq!(next_run(now, four(), Berlin), utc(2026, 11, 22, 3, 0));
        // Summer: 04:00 CEST is 02:00 UTC.
        let now = utc(2026, 7, 1, 2, 30);
        assert_eq!(next_run(now, four(), Berlin), utc(2026, 7, 2, 2, 0));
        // Late evening UTC is already the next local day.
        let now = utc(2026, 11, 21, 23, 30);
        assert_eq!(next_run(now, four(), Berlin), utc(2026, 11, 22, 3, 0));
    }

    #[test]
    fn dst_gap_and_overlap() {
        let two_thirty = NaiveTime::from_hms_opt(2, 30, 0).unwrap();
        // 2026-03-29: 02:30 does not exist in Berlin; 03:30 CEST is used.
        let now = utc(2026, 3, 28, 12, 0);
        assert_eq!(next_run(now, two_thirty, Berlin), utc(2026, 3, 29, 1, 30));
        // 2026-10-25: 02:30 happens twice; the first (CEST) one is used.
        let now = utc(2026, 10, 24, 12, 0);
        let first = next_run(now, two_thirty, Berlin);
        assert_eq!(first, utc(2026, 10, 25, 0, 30));
        // And only once that day: the next run is the day after.
        assert_eq!(
            next_run(first, two_thirty, Berlin),
            utc(2026, 10, 26, 1, 30)
        );
    }

    #[test]
    fn hhmm() {
        assert_eq!(parse_hhmm("04:00"), Some(four()));
        assert_eq!(parse_hhmm(" 4:00 "), Some(four()));
        assert_eq!(parse_hhmm("23:59"), NaiveTime::from_hms_opt(23, 59, 0));
        for bad in [
            "24:00", "4", "04:0", "04:60", "-1:00", "aa:bb", "", "004:00",
        ] {
            assert_eq!(parse_hhmm(bad), None, "{bad}");
        }
    }
}
