//! Which backups to delete. Pure, so the rule is tested without files.
//!
//! Regular backups (daily and manual) follow the usual daily/weekly rule,
//! as in restic's `--keep-daily N --keep-weekly M`: keep the newest backup
//! of each of the last N local days that have one, plus the newest of each
//! of the last M ISO weeks that have one. Pre-migration and pre-restore
//! backups are kept by count, per kind, so a rare schema change never loses
//! its only copy to the daily rule.

use std::collections::HashSet;

use chrono::{DateTime, Datelike, NaiveDate, Utc};
use chrono_tz::Tz;

use crate::name::BackupKind;

/// How many backups to keep.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Policy {
    /// Distinct local days with a regular backup.
    pub keep_daily: usize,
    /// Distinct ISO weeks with a regular backup.
    pub keep_weekly: usize,
    /// Newest pre-migration backups, and separately newest pre-restore
    /// backups.
    pub keep_special: usize,
}

impl Default for Policy {
    fn default() -> Self {
        Self {
            keep_daily: 14,
            keep_weekly: 8,
            keep_special: 5,
        }
    }
}

/// The indices into `backups` that the policy deletes. Days and weeks are
/// counted in `tz`, the zone of the daily schedule.
pub fn to_delete(backups: &[(DateTime<Utc>, BackupKind)], tz: Tz, policy: Policy) -> Vec<usize> {
    let mut order: Vec<usize> = (0..backups.len()).collect();
    // Newest first; ties broken by index, so the result is deterministic.
    order.sort_by(|&a, &b| backups[b].0.cmp(&backups[a].0).then(a.cmp(&b)));

    let mut keep = HashSet::new();
    let regular: Vec<usize> = order
        .iter()
        .copied()
        .filter(|&i| backups[i].1.is_regular())
        .collect();
    let local_day = |i: usize| -> NaiveDate { backups[i].0.with_timezone(&tz).date_naive() };
    keep_newest_per(&regular, policy.keep_daily, local_day, &mut keep);
    keep_newest_per(
        &regular,
        policy.keep_weekly,
        |i| {
            let w = local_day(i).iso_week();
            (w.year(), w.week())
        },
        &mut keep,
    );
    for special in [
        |k: BackupKind| matches!(k, BackupKind::PreMigrate { .. }),
        |k: BackupKind| k == BackupKind::PreRestore,
    ] {
        keep.extend(
            order
                .iter()
                .copied()
                .filter(|&i| special(backups[i].1))
                .take(policy.keep_special),
        );
    }

    let mut out: Vec<usize> = (0..backups.len()).filter(|i| !keep.contains(i)).collect();
    out.sort_unstable();
    out
}

/// Keeps the first (newest) entry of each of the first `n` distinct buckets
/// of `newest_first`.
fn keep_newest_per<K: Eq + std::hash::Hash>(
    newest_first: &[usize],
    n: usize,
    bucket: impl Fn(usize) -> K,
    keep: &mut HashSet<usize>,
) {
    let mut seen = HashSet::new();
    for &i in newest_first {
        if seen.len() >= n {
            break;
        }
        if seen.insert(bucket(i)) {
            keep.insert(i);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{Duration, TimeZone};

    fn day(y: i32, m: u32, d: u32, h: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(y, m, d, h, 0, 0).unwrap()
    }

    fn kept(backups: &[(DateTime<Utc>, BackupKind)], tz: Tz, policy: Policy) -> Vec<usize> {
        let del = to_delete(backups, tz, policy);
        (0..backups.len()).filter(|i| !del.contains(i)).collect()
    }

    /// One daily backup at 03:00 UTC for `n` days, ending on 2026-11-22
    /// (a Sunday), oldest first.
    fn dailies(n: i64) -> Vec<(DateTime<Utc>, BackupKind)> {
        let last = day(2026, 11, 22, 3);
        (0..n)
            .rev()
            .map(|k| (last - Duration::days(k), BackupKind::Daily))
            .collect()
    }

    #[test]
    fn fourteen_daily_and_eight_weekly() {
        let b = dailies(120);
        let keep: Vec<DateTime<Utc>> = kept(&b, Tz::UTC, Policy::default())
            .into_iter()
            .map(|i| b[i].0)
            .collect();
        // The 14 newest days ...
        for k in 0..14 {
            assert!(keep.contains(&(day(2026, 11, 22, 3) - Duration::days(k))));
        }
        // ... plus the newest of each of the last 8 ISO weeks: the Sundays.
        // The two newest weeks are already covered by the dailies, so 6
        // more backups (Sundays of weeks 1 to 7 back, minus week 1).
        let sundays: Vec<_> = (0..8)
            .map(|w| day(2026, 11, 22, 3) - Duration::weeks(w))
            .collect();
        for s in &sundays {
            assert!(keep.contains(s), "{s}");
        }
        assert_eq!(keep.len(), 14 + 6, "{keep:?}");
        // Nothing older than the 8th week survives.
        assert!(keep.iter().all(|t| *t >= sundays[7]));
    }

    #[test]
    fn few_backups_are_all_kept() {
        let b = dailies(5);
        assert!(to_delete(&b, Tz::UTC, Policy::default()).is_empty());
        assert!(to_delete(&[], Tz::UTC, Policy::default()).is_empty());
    }

    #[test]
    fn several_backups_a_day_keep_the_newest() {
        let b = vec![
            (day(2026, 11, 22, 1), BackupKind::Manual),
            (day(2026, 11, 22, 3), BackupKind::Daily),
            (day(2026, 11, 22, 15), BackupKind::Manual),
            (day(2026, 11, 21, 3), BackupKind::Daily),
        ];
        assert_eq!(kept(&b, Tz::UTC, Policy::default()), vec![2, 3]);
    }

    #[test]
    fn weeks_cross_the_year_boundary() {
        // 2026-12-28 (Mon) to 2027-01-03 (Sun) is ISO week 53 of 2026; the
        // next week is week 1 of 2027.
        let b = vec![
            (day(2026, 12, 29, 3), BackupKind::Daily),
            (day(2027, 1, 2, 3), BackupKind::Daily),
            (day(2027, 1, 5, 3), BackupKind::Daily),
        ];
        let policy = Policy {
            keep_daily: 1,
            keep_weekly: 2,
            keep_special: 5,
        };
        // Week 1/2027 → Jan 5 (also the daily); week 53/2026 → Jan 2.
        assert_eq!(kept(&b, Tz::UTC, policy), vec![1, 2]);
    }

    #[test]
    fn days_are_local_days() {
        // 23:30 UTC on Nov 21 is already Nov 22 in Berlin.
        let late = Utc.with_ymd_and_hms(2026, 11, 21, 23, 30, 0).unwrap();
        let b = vec![
            (late, BackupKind::Manual),
            (day(2026, 11, 22, 3), BackupKind::Daily),
        ];
        let policy = Policy {
            keep_daily: 2,
            keep_weekly: 0,
            keep_special: 0,
        };
        assert_eq!(kept(&b, Tz::UTC, policy), vec![0, 1]);
        assert_eq!(kept(&b, chrono_tz::Europe::Berlin, policy), vec![1]);
    }

    #[test]
    fn special_backups_are_kept_by_count_per_kind() {
        let mut b = dailies(1);
        for k in 0..7 {
            b.push((
                day(2026, 1, 1, 0) + Duration::days(k),
                BackupKind::PreMigrate { version: k },
            ));
            b.push((
                day(2025, 1, 1, 0) + Duration::days(k),
                BackupKind::PreRestore,
            ));
        }
        let keep = kept(&b, Tz::UTC, Policy::default());
        let count = |f: fn(BackupKind) -> bool| keep.iter().filter(|&&i| f(b[i].1)).count();
        assert_eq!(count(|k| matches!(k, BackupKind::PreMigrate { .. })), 5);
        assert_eq!(count(|k| k == BackupKind::PreRestore), 5);
        assert_eq!(count(BackupKind::is_regular), 1);
        // The oldest two of each kind go.
        let del = to_delete(&b, Tz::UTC, Policy::default());
        assert_eq!(del, vec![1, 2, 3, 4]);
    }
}
