# 0015 – Backups

- **Status:** accepted
- **Date:** 2026-11-22
- **Ticket:** P4

## Context

All data is one SQLite file (WAL mode) on the Pi. Watchtower deploys every green merge without anyone watching, so a bad migration, a broken SD card or an admin mistake could lose everything. P4 asks for backups built into the app (no extra container):

- daily at a configurable local time;
- a backup before every migration, and no migration if it fails;
- an integrity check for every backup;
- retention of 14 daily and 8 weekly backups;
- an optional second target;
- a restore CLI;
- `/admin backup now`, plus backup info in `/admin status`.

Constraints:

- the app keeps serving during a backup (under 1 s of blocking);
- the image is distroless (no shell, no tzdata);
- arm64 is cross-compiled with Debian's cross gcc (ADR 0008).

## Decision

1. **A new crate `backup`**, between `store` and `bot`/`app`. It reads the clock and the file system and holds a run lock and the last error, so it doesn't fit `store`, whose repositories never read the clock. The bot gets a `BackupService` handle in `BotDeps`.
2. **One backup:**
   - `VACUUM INTO '<file>.vacuum.tmp'` on a pooled connection. In WAL mode this is one read transaction, so writers keep going. Measured on amd64: a 50 MB database backs up in 0.8 s, and concurrent writes stay under 10 ms.
   - Then `PRAGMA integrity_check` on the snapshot, which is first set to rollback-journal mode so it stands alone.
   - Then zstd level 3 (crate `zstd`; its C code builds with the existing cross gcc, like the bundled SQLite), fsync, and a rename into place.
   - Unfinished files end in `.tmp`. They are never taken for backups and are removed at the next start.
   - A tokio mutex allows one run at a time. A second `/admin backup now` gets "already running".
3. **The directory is the index.** File names carry the UTC time and the kind, e.g. `pickem-20261122T030000Z-daily.db.zst` or `pickem-…-premigrate-v0006.db.zst`, where `v0006` is the newest applied migration. There is no database table: a table would be inside the backups it describes, and last time and size come from the newest file, so they survive restarts. The last error is kept in memory only.
4. **Retention** (pure function, restic semantics):
   - daily and manual backups: keep the newest backup of each of the last `BACKUP_KEEP_DAILY` (14) local days, plus the newest of each of the last `BACKUP_KEEP_WEEKLY` (8) ISO weeks;
   - pre-migration and pre-restore backups: keep the newest `BACKUP_KEEP_SPECIAL` (5) of each kind, so a rare schema change never loses its only copy to the daily rule.
   - Pruning runs after every backup, in both directories.
5. **Schedule:**
   - `BACKUP_TIME` (`HH:MM`, default `04:00`, or `off`) in the zone `TZ`: an IANA name parsed with `chrono-tz` (the image has no tzdata), UTC when unset. The owner chose `TZ` over a separate setting or the guild's digest time zone.
   - DST gaps use the time an hour later; repeated times use the first occurrence.
   - If no regular backup exists since the last slot (the Pi was off, or this is the first start), one runs right after start.
   - The loop sleeps at most 10 min at a time and re-reads the wall clock, because a Pi has no RTC and its clock may jump when NTP syncs after boot.
6. **Pre-migration backup:**
   - `backup::open_db` replaces `store::open` for every command that opens the database (`run`, `reconcile`, `rescore`, `rules`).
   - It compares the migrator with `_sqlx_migrations` (`store::pending_migrations`). If migrations are pending on a database that holds data, it writes the backup first. If that fails, it returns an error without migrating, the process exits 1, and compose/Watchtower keep restarting it with the old data untouched.
   - A fresh, empty database needs no backup.
7. **Second target** `BACKUP_EXTRA_DIR`: each finished file is copied (temp file, then rename) and that directory is pruned too. A failed copy is logged and shown in status, but the primary backup counts.
8. **Restore** `app restore <file>` (the app must be stopped):
   - unpack next to the database and run the integrity check; stop with nothing changed if it fails;
   - back up the current database as `prerestore`, or move it aside as `pickem.db.broken-<time>` if it can't be read;
   - rename the restored file into place and delete stale `-wal`/`-shm` files, which would otherwise corrupt it;
   - don't migrate: the next start does that, with its own pre-migration backup.
9. **Alerts:** until P5 exists, a failure is logged at `ERROR` with `backup_failed=true` and shown with ⚠️ in `/admin status`. `/admin backup now` defers its reply, because a backup may take longer than Discord's 3 s.

## Consequences

- Backups need no setup beyond the existing `./data` volume. A NAS copy is one volume line and one variable.
- A failed pre-migration backup (e.g. a full disk) stops the app until fixed. That is deliberate: no data is at risk, and rolling back with `IMAGE_TAG` brings it up again.
- If shutdown arrives during a backup, the backup is abandoned; its temp files are removed at the next start.
- Daily backups count days in `TZ`. Changing `TZ` shifts the time of day but doesn't break retention.
- P5 can read `BackupService::status()` for its panel and hook its alert where `backup_failed` is logged.
- "Under 30 s on a Pi 4" can only be confirmed on the Pi: `cargo test --release -p backup --test nonblocking -- --ignored --nocapture`, or the duration that `/admin backup now` reports.
