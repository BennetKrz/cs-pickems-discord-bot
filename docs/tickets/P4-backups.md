# P4 – Backups

- **Epic:** Proposed extras (optional)
- **Status:** proposed (optional)
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

- [ ] Daily backups are created and pruned per the retention rule.
- [ ] A test with a dummy migration shows the pre-migration backup is written first.
- [ ] A round trip works: back up, change data, restore, and the data matches the backup.
- [ ] Backing up a 50 MB database takes under 30 s on a Pi 4 and never blocks a request for more than 1 s.
