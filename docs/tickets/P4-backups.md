# P4 – Backups

- **Epic:** Proposed extras (optional)
- **Status:** done
- **Index:** [all tickets](README.md)

**Goal:** no data loss through a bad update, a broken SD card or a mistake.

**Depends on:** T16

**Scope**

- Built into the app, no extra container. Daily at a configurable local time (default 04:00): `VACUUM INTO` a new file in `/data/backups/`, consistent while the app keeps running, then compress with zstd.
- Retention: 14 daily and 8 weekly backups, configurable.
- **Pre-migration backup:** at startup, when migrations are pending, back up first; the file name carries the schema version. If that backup fails, the app does not migrate and exits with an error. Watchtower then keeps restarting, and the old data stays untouched.
- Each backup is checked with `PRAGMA integrity_check`. A failure is logged and alerted (P5 when present).
- Optional second target `BACKUP_EXTRA_DIR`, e.g. a mounted NAS share.
- **Restore:** `app restore <file>` CLI, documented as `docker compose stop app`, then `docker compose run --rm app restore …`, then start.
- `/admin backup now`; last backup time and size in `/admin status`.

**Acceptance criteria**

- [x] Daily backups are created and pruned per the retention rule.
- [x] A test with a dummy migration shows the pre-migration backup is written first.
- [x] A round trip works: back up, change data, restore, and the data matches the backup.
- [x] Backing up a 50 MB database takes under 30 s on a Pi 4 and never blocks a request for more than 1 s.

**Notes**

- Decisions: [ADR 0015](../adr/0015-backups.md). Code in the new crate `backend/crates/backup`. The app wires it in `app::open_db` (every command that opens the database), the scheduler task and `app restore`. The bot has `/admin backup now` and a *Backups* section in `/admin status`.
- Settings: `BACKUP_TIME` (`HH:MM` or `off`), `TZ` (IANA name; UTC when unset; chosen by the owner), `BACKUP_DIR` (default `backups/` next to the database), `BACKUP_EXTRA_DIR`, `BACKUP_KEEP_DAILY` (14), `BACKUP_KEEP_WEEKLY` (8), `BACKUP_KEEP_SPECIAL` (5). The `backup_*` keys also work in `config.toml`.
- Beyond the scope, decided while building:
  - pre-migration and pre-restore backups are kept by count (newest 5 each), outside the 14/8 rule (owner's choice);
  - a missed daily slot (Pi off at 04:00, first start) is caught up right after start;
  - restore first backs up the database it replaces (`prerestore`), or moves it aside if it is unreadable.
- Tests:
  - `backup/src/{name,retention,schedule}.rs`: file names, the 14/8 rule (year boundary, local days, several per day, specials by count), DST gaps and repeats;
  - `backup/tests/backup.rs`: write, check and read back; extra dir (copy and failure); pruning in both directories; one run at a time; failure in status and cleared by the next success; scheduler catch-up, stop and `off` (**AC1**);
  - `backup/tests/premigration.rs`: a runtime migrator with the real migrations plus `0007_dummy.sql`. The backup is `…-premigrate-v0006…` and lacks the dummy table, while the live DB has it. When the backup fails, nothing is migrated. No backup on a fresh DB or with nothing pending (**AC2**);
  - `backup/tests/restore.rs`: the round trip and undoing it with the safety backup; a corrupt or non-SQLite file changes nothing; a broken current DB is moved aside; restore into an empty data dir (**AC3**);
  - `backup/tests/nonblocking.rs`: a backup under a write and read load (10 MB in the normal suite; 50 MB `#[ignore]`d) (**AC4**, partly);
  - `app/tests/process.rs`: the real binary takes the catch-up backup at start, and `app restore <bare name>` brings it back;
  - `app/src/config.rs`: defaults, env over file, `off`, `TZ` forms, errors;
  - `bot`: the status text and the command list.
- Checked locally on 22 Nov 2026 (amd64, release build, `cargo test --release -p backup --test nonblocking -- --include-ignored --nocapture`):
  - 50 MB backs up in 0.79 s (28.8 MB compressed; the test data is random hex, real data compresses better), and the slowest of 133 concurrent write+read requests took 7 ms;
  - 10 MB backs up in 0.25 s.
  - the arm64 image cross-builds with the `zstd` C code (`docker buildx build --platform linux/arm64 .`; aarch64 ELF; image 25 MB), so no Dockerfile change was needed.
- Confirmed by the owner on the Pi 4 on 22 Nov 2026 (AC4).
- Follow-up for P5: read `BackupService::status()` for the backups panel, and raise the alert where `backup_failed=true` is logged (in `backup::BackupService::run` and `backup::open_db`).
