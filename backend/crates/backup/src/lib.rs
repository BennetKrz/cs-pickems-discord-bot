//! Database backups (P4), built into the app.
//!
//! - **Daily** at a local time ([`schedule`]), and **manual** via
//!   `/admin backup now`: [`BackupService`].
//! - **Before migrations** at start-up: [`open_db`]. If that backup fails,
//!   the database is not migrated and the app exits with an error.
//! - Each backup is a `VACUUM INTO` snapshot, checked with
//!   `PRAGMA integrity_check`, compressed with zstd and renamed into place
//!   ([`snapshot`]); optionally copied to a second directory; then old ones
//!   are pruned ([`retention`]).
//! - **Restore** with `app restore <file>`: [`restore()`].
//!
//! There is no database table for backups: the backup directory is the
//! index (see [`name`]). See `docs/adr/0015-backups.md`.

pub mod name;
mod restore;
pub mod retention;
pub mod schedule;
mod snapshot;

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use chrono::{DateTime, NaiveTime, Utc};
use chrono_tz::Tz;
use sqlx::migrate::Migrator;
use store::SqlitePool;
use tokio_util::sync::CancellationToken;

pub use name::BackupKind;
pub use restore::{RestoreReport, restore};
pub use retention::Policy;

/// Where and when to back up, and how many backups to keep.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackupConfig {
    /// `BACKUP_DIR`; by default `backups/` next to the database file.
    pub dir: PathBuf,
    /// `BACKUP_EXTRA_DIR`: a second copy of every backup, e.g. a NAS share.
    pub extra_dir: Option<PathBuf>,
    /// `BACKUP_TIME`, local; `None` turns the daily backup off (the
    /// pre-migration backup always runs).
    pub time: Option<NaiveTime>,
    /// The zone of `time` and of the days in the retention rule: `TZ`.
    pub tz: Tz,
    pub policy: Policy,
}

impl BackupConfig {
    /// The defaults: daily at 04:00 UTC, 14 daily, 8 weekly, 5 special.
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self {
            dir: dir.into(),
            extra_dir: None,
            time: NaiveTime::from_hms_opt(4, 0, 0),
            tz: Tz::UTC,
            policy: Policy::default(),
        }
    }
}

/// One finished backup file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackupInfo {
    pub path: PathBuf,
    pub file_name: String,
    pub kind: BackupKind,
    pub created_at: DateTime<Utc>,
    /// Compressed size in bytes.
    pub size: u64,
}

/// What a successful run did.
#[derive(Debug, Clone)]
pub struct BackupOutcome {
    pub backup: BackupInfo,
    pub duration: std::time::Duration,
    /// Old backups deleted by retention (both directories).
    pub pruned: usize,
    /// The copy to `extra_dir` failed; the primary backup is fine.
    pub extra_error: Option<String>,
}

/// Why a backup failed.
#[derive(Debug, thiserror::Error)]
pub enum BackupError {
    #[error("a backup is already running")]
    AlreadyRunning,
    #[error("{action}: {source}")]
    Io {
        action: String,
        #[source]
        source: std::io::Error,
    },
    #[error("database error: {0}")]
    Db(#[source] sqlx::Error),
    #[error("integrity check failed: {0}")]
    Integrity(String),
    #[error("path is not valid UTF-8: {0}")]
    Path(String),
    #[error("{0}")]
    Restore(String),
}

/// A failed run, for `/admin status`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FailedRun {
    pub at: DateTime<Utc>,
    pub message: String,
}

/// Backup health for `/admin status`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackupStatus {
    /// The newest backup of any kind in the backup directory.
    pub last: Option<BackupInfo>,
    /// The last run's problem; cleared by a fully successful run.
    pub last_error: Option<FailedRun>,
    /// The next daily run; `None` when daily backups are off.
    pub next_run: Option<DateTime<Utc>>,
    pub running: bool,
}

/// Runs backups, one at a time, and remembers the last failure.
#[derive(Clone)]
pub struct BackupService {
    inner: Arc<Inner>,
}

struct Inner {
    pool: SqlitePool,
    config: BackupConfig,
    running: tokio::sync::Mutex<()>,
    last_error: Mutex<Option<FailedRun>>,
}

/// After writing a backup: copy it to the extra directory and prune both
/// directories. Problems here don't fail the backup; they are returned as
/// a message.
async fn finish(info: &BackupInfo, config: &BackupConfig) -> (usize, Option<String>) {
    let mut pruned = 0;
    let mut problem = None;
    match snapshot::prune(&config.dir, config.tz, config.policy).await {
        Ok(d) => pruned += d.len(),
        Err(e) => {
            tracing::warn!("cannot prune old backups: {e}");
        }
    }
    if let Some(extra) = &config.extra_dir {
        match snapshot::copy_to(info, extra).await {
            Ok(()) => match snapshot::prune(extra, config.tz, config.policy).await {
                Ok(d) => pruned += d.len(),
                Err(e) => tracing::warn!("cannot prune old backups in BACKUP_EXTRA_DIR: {e}"),
            },
            Err(e) => {
                let msg = format!("copy to BACKUP_EXTRA_DIR failed: {e}");
                tracing::error!(backup_failed = true, file = %info.file_name, "{msg}");
                problem = Some(msg);
            }
        }
    }
    (pruned, problem)
}

impl BackupService {
    pub fn new(pool: SqlitePool, config: BackupConfig) -> Self {
        Self {
            inner: Arc::new(Inner {
                pool,
                config,
                running: tokio::sync::Mutex::new(()),
                last_error: Mutex::new(None),
            }),
        }
    }

    pub fn config(&self) -> &BackupConfig {
        &self.inner.config
    }

    /// Takes one backup now, unless one is already running.
    pub async fn run(&self, kind: BackupKind) -> Result<BackupOutcome, BackupError> {
        let Ok(_guard) = self.inner.running.try_lock() else {
            return Err(BackupError::AlreadyRunning);
        };
        let started = Instant::now();
        let now = Utc::now();
        let config = &self.inner.config;
        let result = match snapshot::create(&self.inner.pool, &config.dir, kind, now).await {
            Ok(info) => {
                let (pruned, extra_error) = finish(&info, config).await;
                Ok(BackupOutcome {
                    backup: info,
                    duration: started.elapsed(),
                    pruned,
                    extra_error,
                })
            }
            Err(e) => Err(e),
        };
        let failure = match &result {
            Ok(o) => {
                tracing::info!(
                    kind = %kind.label(),
                    file = %o.backup.file_name,
                    bytes = o.backup.size,
                    ms = o.duration.as_millis() as u64,
                    pruned = o.pruned,
                    "backup written"
                );
                o.extra_error.clone()
            }
            Err(e) => {
                tracing::error!(backup_failed = true, kind = %kind.label(), "backup failed: {e}");
                Some(e.to_string())
            }
        };
        *self.lock_error() = failure.map(|message| FailedRun { at: now, message });
        result
    }

    fn lock_error(&self) -> std::sync::MutexGuard<'_, Option<FailedRun>> {
        self.inner
            .last_error
            .lock()
            .unwrap_or_else(|e| e.into_inner())
    }

    /// The newest backup, the last problem and the next daily run.
    pub async fn status(&self) -> BackupStatus {
        let config = &self.inner.config;
        let last = match snapshot::list(&config.dir).await {
            Ok(mut all) => all.pop(),
            Err(e) => {
                tracing::warn!("cannot list backups: {e}");
                None
            }
        };
        BackupStatus {
            last,
            last_error: self.lock_error().clone(),
            next_run: config
                .time
                .map(|t| schedule::next_run(Utc::now(), t, config.tz)),
            running: self.inner.running.try_lock().is_err(),
        }
    }

    /// Runs the daily backup until `stop`. Catches up at once when the last
    /// slot passed without a regular backup (e.g. the Pi was off at 04:00).
    ///
    /// Sleeps in short steps and re-reads the wall clock, because a Pi has
    /// no real-time clock: right after boot the time may still jump when
    /// NTP syncs.
    pub async fn run_schedule(self, stop: CancellationToken) {
        const STEP: std::time::Duration = std::time::Duration::from_secs(600);
        let config = &self.inner.config;
        snapshot::clean_tmp(&config.dir).await;
        let Some(at) = config.time else {
            tracing::info!("daily backups are off (BACKUP_TIME=off)");
            return;
        };
        let tz = config.tz;
        tracing::info!(
            next = %schedule::next_run(Utc::now(), at, tz),
            dir = %config.dir.display(),
            "daily backups scheduled"
        );

        let previous = schedule::previous_run(Utc::now(), at, tz);
        let have_recent = snapshot::list(&config.dir)
            .await
            .map(|all| {
                all.iter()
                    .any(|b| b.kind.is_regular() && b.created_at >= previous)
            })
            .unwrap_or(false);
        let mut due = if have_recent {
            schedule::next_run(Utc::now(), at, tz)
        } else {
            tracing::info!("no backup since the last daily slot; backing up now");
            Utc::now()
        };

        loop {
            let now = Utc::now();
            if now >= due {
                tokio::select! {
                    _ = stop.cancelled() => return,
                    _ = self.run(BackupKind::Daily) => {}
                }
                due = schedule::next_run(Utc::now(), at, tz);
                continue;
            }
            let wait = (due - now).to_std().unwrap_or_default().min(STEP);
            tokio::select! {
                _ = stop.cancelled() => return,
                _ = tokio::time::sleep(wait) => {}
            }
        }
    }
}

/// Why the database could not be opened.
#[derive(Debug, thiserror::Error)]
pub enum OpenError {
    #[error(transparent)]
    Store(#[from] store::StoreError),
    #[error("cannot check for pending migrations: {0}")]
    Check(#[source] sqlx::Error),
    #[error(
        "the backup before migrating (schema v{version}) failed, so the database was not \
         migrated: {source}"
    )]
    Backup {
        version: i64,
        #[source]
        source: BackupError,
    },
}

/// Opens the database and applies `migrator`'s pending migrations, but
/// first writes a pre-migration backup, named after the schema version it
/// holds. If that backup fails, nothing is migrated and an error returns.
/// A fresh, empty database needs no backup.
pub async fn open_db(
    database_url: &str,
    config: &BackupConfig,
    migrator: &Migrator,
) -> Result<SqlitePool, OpenError> {
    let pool = store::connect(database_url).await?;
    let pending = match store::pending_migrations(&pool, migrator).await {
        Ok(p) => p,
        Err(e) => {
            pool.close().await;
            return Err(OpenError::Check(e));
        }
    };
    if let Some(version) = pending {
        tracing::info!(
            version,
            "migrations are pending; backing up the database first"
        );
        let kind = BackupKind::PreMigrate { version };
        match snapshot::create(&pool, &config.dir, kind, Utc::now()).await {
            Ok(info) => {
                tracing::info!(file = %info.file_name, bytes = info.size, "pre-migration backup written");
                finish(&info, config).await;
            }
            Err(source) => {
                tracing::error!(
                    backup_failed = true,
                    version,
                    "pre-migration backup failed: {source}"
                );
                pool.close().await;
                return Err(OpenError::Backup { version, source });
            }
        }
    }
    if let Err(e) = store::migrate_with(&pool, migrator).await {
        pool.close().await;
        return Err(e.into());
    }
    Ok(pool)
}
