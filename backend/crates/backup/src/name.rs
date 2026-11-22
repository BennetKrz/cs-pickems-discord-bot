//! Backup file names: `pickem-<UTC time>-<kind>.db.zst`, for example
//! `pickem-20261122T030000Z-daily.db.zst`. Names sort by time, and the kind
//! and time can be read back from the name alone, so the backup directory
//! is its own index.

use chrono::{DateTime, NaiveDateTime, Utc};

/// Every backup file starts with this.
pub const PREFIX: &str = "pickem-";
/// Every finished backup file ends with this. Files still being written
/// end in `.tmp` and are never taken for backups.
pub const SUFFIX: &str = ".db.zst";
const TIME_FORMAT: &str = "%Y%m%dT%H%M%SZ";
/// Length of a formatted time, e.g. `20261122T030000Z`.
const TIME_LEN: usize = 16;

/// Why a backup was taken. Retention treats `Daily` and `Manual` as regular
/// backups; the other two are kept by count (see [`crate::retention`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BackupKind {
    /// The scheduled daily backup.
    Daily,
    /// `/admin backup now`.
    Manual,
    /// Before migrations ran at start-up; `version` is the schema version
    /// the backup holds (the newest applied migration).
    PreMigrate { version: i64 },
    /// Of the database that `app restore` replaced.
    PreRestore,
}

impl BackupKind {
    /// Daily and manual backups, pruned by the daily/weekly rule.
    pub fn is_regular(self) -> bool {
        matches!(self, BackupKind::Daily | BackupKind::Manual)
    }

    /// The kind as it appears in file names and messages.
    pub fn label(self) -> String {
        match self {
            BackupKind::Daily => "daily".to_owned(),
            BackupKind::Manual => "manual".to_owned(),
            BackupKind::PreMigrate { version } => format!("premigrate-v{version:04}"),
            BackupKind::PreRestore => "prerestore".to_owned(),
        }
    }

    fn parse(s: &str) -> Option<Self> {
        match s {
            "daily" => Some(BackupKind::Daily),
            "manual" => Some(BackupKind::Manual),
            "prerestore" => Some(BackupKind::PreRestore),
            _ => {
                let v = s.strip_prefix("premigrate-v")?;
                if v.is_empty() || !v.bytes().all(|b| b.is_ascii_digit()) {
                    return None;
                }
                Some(BackupKind::PreMigrate {
                    version: v.parse().ok()?,
                })
            }
        }
    }
}

/// The file name of a backup taken at `at` (whole seconds, UTC).
pub fn format(at: DateTime<Utc>, kind: BackupKind) -> String {
    format!(
        "{PREFIX}{}-{}{SUFFIX}",
        at.format(TIME_FORMAT),
        kind.label()
    )
}

/// Reads time and kind back from a file name; `None` for anything that is
/// not a finished backup.
pub fn parse(name: &str) -> Option<(DateTime<Utc>, BackupKind)> {
    let rest = name.strip_prefix(PREFIX)?.strip_suffix(SUFFIX)?;
    if rest.len() <= TIME_LEN + 1 || !rest.is_char_boundary(TIME_LEN) {
        return None;
    }
    let (time, kind) = rest.split_at(TIME_LEN);
    let kind = kind.strip_prefix('-')?;
    let at = NaiveDateTime::parse_from_str(time, TIME_FORMAT)
        .ok()?
        .and_utc();
    Some((at, BackupKind::parse(kind)?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn at() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 11, 22, 3, 0, 0).unwrap()
    }

    #[test]
    fn names_round_trip() {
        for kind in [
            BackupKind::Daily,
            BackupKind::Manual,
            BackupKind::PreMigrate { version: 6 },
            BackupKind::PreMigrate { version: 20261122 },
            BackupKind::PreRestore,
        ] {
            let name = format(at(), kind);
            assert_eq!(parse(&name), Some((at(), kind)), "{name}");
        }
        assert_eq!(
            format(at(), BackupKind::Daily),
            "pickem-20261122T030000Z-daily.db.zst"
        );
        assert_eq!(
            format(at(), BackupKind::PreMigrate { version: 6 }),
            "pickem-20261122T030000Z-premigrate-v0006.db.zst"
        );
    }

    #[test]
    fn foreign_and_unfinished_files_are_not_backups() {
        for name in [
            "pickem.db",
            "pickem-20261122T030000Z-daily.db.zst.tmp",
            "pickem-20261122T030000Z-daily.db.vacuum.tmp",
            "pickem-20261122T030000Z-weekly.db.zst",
            "pickem-20261122T030000Z-premigrate-v.db.zst",
            "pickem-20261122T030000Z-premigrate-vx1.db.zst",
            "pickem-20261322T030000Z-daily.db.zst",
            "pickem-20261122T030000Zdaily.db.zst",
            "pickem-.db.zst",
            "other-20261122T030000Z-daily.db.zst",
            "pickem-2026112ä030000Z-daily.db.zst",
        ] {
            assert_eq!(parse(name), None, "{name}");
        }
    }
}
