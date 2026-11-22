//! Database access: the SQLite pool, migrations and repositories.
//!
//! Repositories are free functions per area (`teams`, `matches`, `picks`, …).
//! Single-statement functions take any [`sqlx::SqliteExecutor`] (a pool, a
//! connection or a transaction), so callers can compose them inside one
//! transaction. Functions that need several statements take an
//! [`sqlx::Acquire`] and open their own (nested) transaction.
//!
//! Repositories never read the clock: callers pass `now`, so a fake clock
//! (T05) works. Conventions are recorded in `docs/adr/0003-database-schema.md`.

pub mod audit;
pub mod bot;
pub mod events;
pub mod guilds;
pub mod leaderboard;
pub mod matches;
pub mod pending_views;
pub mod picks;
pub mod provenance;
pub mod refs;
mod row;
pub mod scoring;
pub mod sessions;
pub mod sync_state;
pub mod teams;
mod time;
pub mod unlinked;
pub mod users;

use std::path::Path;
use std::str::FromStr;
use std::time::Duration;

use sqlx::migrate::Migrator;
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous};

pub use sqlx::SqlitePool;
pub use time::{Ts, format_ts};

/// Result type of the repositories.
pub type DbResult<T> = Result<T, sqlx::Error>;

/// The embedded migrations from `crates/store/migrations`.
static MIGRATOR: Migrator = sqlx::migrate!();

/// Errors from opening or using the database.
#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("invalid DATABASE_URL: {0}")]
    InvalidUrl(String),
    #[error("cannot create database directory {path}: {source}")]
    CreateDir {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("cannot open database: {0}")]
    Connect(#[source] sqlx::Error),
    #[error("cannot migrate database: {0}")]
    Migrate(#[source] sqlx::migrate::MigrateError),
}

/// Applies all pending migrations. Already applied ones are skipped, so this
/// is safe on every start. Fails if an applied migration was edited.
pub async fn migrate(pool: &SqlitePool) -> Result<(), StoreError> {
    migrate_with(pool, &MIGRATOR).await
}

/// Applies the pending migrations of `migrator`. The app uses [`migrator`];
/// tests pass one with extra (dummy) migrations.
pub async fn migrate_with(pool: &SqlitePool, migrator: &Migrator) -> Result<(), StoreError> {
    migrator.run(pool).await.map_err(StoreError::Migrate)
}

/// The embedded migrations of the app.
pub fn migrator() -> &'static Migrator {
    &MIGRATOR
}

/// Whether `migrator` would change the database, and from which schema
/// version: `Some(current)` when migrations are pending on a database that
/// already holds data (`current` is the newest applied version, 0 if none).
/// `None` when nothing is pending, or when the database is still empty, so
/// there is nothing to lose (P4 backs up before migrating on `Some`).
pub async fn pending_migrations(pool: &SqlitePool, migrator: &Migrator) -> DbResult<Option<i64>> {
    let tables: Vec<String> = sqlx::query_scalar(
        "SELECT name FROM sqlite_schema WHERE type = 'table' AND name NOT LIKE 'sqlite_%'",
    )
    .fetch_all(pool)
    .await?;
    if tables.is_empty() {
        return Ok(None);
    }
    let applied: Vec<i64> = if tables.iter().any(|t| t == "_sqlx_migrations") {
        sqlx::query_scalar("SELECT version FROM _sqlx_migrations WHERE success = 1")
            .fetch_all(pool)
            .await?
    } else {
        Vec::new()
    };
    let pending = migrator
        .iter()
        .filter(|m| !m.migration_type.is_down_migration())
        .any(|m| !applied.contains(&m.version));
    Ok(pending.then(|| applied.iter().copied().max().unwrap_or(0)))
}

/// The database file behind a `sqlite:` URL; `None` for an in-memory one.
pub fn db_path(database_url: &str) -> Result<Option<std::path::PathBuf>, StoreError> {
    if !database_url.starts_with("sqlite:") {
        return Err(StoreError::InvalidUrl(
            "expected a sqlite: URL such as sqlite://data/pickem.db".to_owned(),
        ));
    }
    let options = SqliteConnectOptions::from_str(database_url)
        .map_err(|e| StoreError::InvalidUrl(e.to_string()))?;
    let path = options.get_filename();
    Ok((!is_in_memory(path)).then(|| path.to_path_buf()))
}

/// Opens the pool and applies pending migrations: what the app does at start.
pub async fn open(database_url: &str) -> Result<SqlitePool, StoreError> {
    let pool = connect(database_url).await?;
    migrate(&pool).await?;
    Ok(pool)
}

/// Opens a SQLite pool for `database_url`.
///
/// Creates the database file (and its parent directory) if missing and sets
/// `journal_mode=WAL`, `foreign_keys=ON` and `busy_timeout=5000` on every
/// connection.
pub async fn connect(database_url: &str) -> Result<SqlitePool, StoreError> {
    // sqlx treats anything it can't parse as a URL as a file name, so a
    // typo such as `postgres://…` would silently create a stray file.
    if !database_url.starts_with("sqlite:") {
        return Err(StoreError::InvalidUrl(
            "expected a sqlite: URL such as sqlite://data/pickem.db".to_owned(),
        ));
    }
    let options = SqliteConnectOptions::from_str(database_url)
        .map_err(|e| StoreError::InvalidUrl(e.to_string()))?
        .create_if_missing(true)
        .journal_mode(SqliteJournalMode::Wal)
        .synchronous(SqliteSynchronous::Normal)
        .foreign_keys(true)
        .busy_timeout(Duration::from_millis(5000));

    if let Some(dir) = options.get_filename().parent()
        && !dir.as_os_str().is_empty()
        && !is_in_memory(options.get_filename())
    {
        std::fs::create_dir_all(dir).map_err(|source| StoreError::CreateDir {
            path: dir.display().to_string(),
            source,
        })?;
    }

    SqlitePoolOptions::new()
        .max_connections(8)
        .connect_with(options)
        .await
        .map_err(StoreError::Connect)
}

fn is_in_memory(path: &Path) -> bool {
    path.as_os_str() == ":memory:"
}

/// Starts a transaction that will write, with `BEGIN IMMEDIATE`.
///
/// SQLite allows one writer at a time. A plain (deferred) transaction that
/// reads first and writes later fails at once with "database is locked" if
/// another connection wrote in between; `busy_timeout` does not help there.
/// An immediate transaction takes the write lock up front and so waits for
/// `busy_timeout` instead. Use it for every read-then-write transaction
/// that may run alongside other writers (sync, lock ticker, admin, picks).
pub async fn begin_write(pool: &SqlitePool) -> DbResult<sqlx::Transaction<'static, sqlx::Sqlite>> {
    pool.begin_with("BEGIN IMMEDIATE").await
}

/// Checks that the database answers a trivial query.
pub async fn ping(pool: &SqlitePool) -> Result<(), sqlx::Error> {
    sqlx::query("SELECT 1").execute(pool).await.map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn connect_creates_file_with_wal_and_pragmas() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested/dir/test.db");
        let url = format!("sqlite://{}", path.display());

        let pool = connect(&url).await.unwrap();
        ping(&pool).await.unwrap();
        assert!(path.exists());

        let (mode,): (String,) = sqlx::query_as("PRAGMA journal_mode")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(mode, "wal");
        let (fk,): (i64,) = sqlx::query_as("PRAGMA foreign_keys")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(fk, 1);
        let (timeout,): (i64,) = sqlx::query_as("PRAGMA busy_timeout")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(timeout, 5000);
        pool.close().await;
    }

    #[tokio::test]
    async fn ping_fails_on_closed_pool() {
        let pool = connect("sqlite::memory:").await.unwrap();
        pool.close().await;
        assert!(ping(&pool).await.is_err());
    }

    #[tokio::test]
    async fn invalid_url_is_reported() {
        let err = connect("postgres://nope").await.unwrap_err();
        assert!(matches!(err, StoreError::InvalidUrl(_)));
    }
}
