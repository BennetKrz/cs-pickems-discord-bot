//! The pre-migration backup: written before a (dummy) migration runs, and
//! no migration without it.

mod common;

use std::path::Path;

use backup::{BackupKind, OpenError, name, open_db};
use common::{files, open_backup, table_exists, team_names, test_db};
use sqlx::migrate::Migrator;

const DUMMY: &str = "p4_dummy";

/// The app's migrations plus `0007_dummy.sql`, which adds a table.
async fn migrator_with_dummy(dir: &Path) -> Migrator {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("../store/migrations");
    let dst = dir.join("migrations");
    std::fs::create_dir_all(&dst).unwrap();
    for entry in std::fs::read_dir(src).unwrap() {
        let entry = entry.unwrap();
        std::fs::copy(entry.path(), dst.join(entry.file_name())).unwrap();
    }
    std::fs::write(
        dst.join("0007_dummy.sql"),
        format!("CREATE TABLE {DUMMY} (id INTEGER PRIMARY KEY) STRICT;\n"),
    )
    .unwrap();
    Migrator::new(dst).await.unwrap()
}

async fn applied_version(pool: &store::SqlitePool) -> i64 {
    sqlx::query_scalar("SELECT MAX(version) FROM _sqlx_migrations WHERE success = 1")
        .fetch_one(pool)
        .await
        .unwrap()
}

#[tokio::test]
async fn the_backup_is_written_before_migrating() {
    let db = test_db().await;
    let current = applied_version(&db.pool).await;
    assert_eq!(current, 6);
    db.pool.close().await;
    let migrator = migrator_with_dummy(db.dir.path()).await;
    let config = db.config();

    let pool = open_db(&db.url(), &config, &migrator).await.unwrap();

    // The live database is migrated ...
    assert!(table_exists(&pool, DUMMY).await);
    assert_eq!(applied_version(&pool).await, 7);
    // ... and one backup named after the old schema version exists, taken
    // before the migration: it has the data but not the new table.
    let backups = files(&config.dir);
    assert_eq!(backups.len(), 1, "{backups:?}");
    assert!(
        backups[0].ends_with("-premigrate-v0006.db.zst"),
        "{backups:?}"
    );
    assert_eq!(
        name::parse(&backups[0]).unwrap().1,
        BackupKind::PreMigrate { version: 6 }
    );
    let copy = open_backup(&config.dir.join(&backups[0]), db.dir.path()).await;
    assert!(!table_exists(&copy, DUMMY).await);
    assert_eq!(applied_version(&copy).await, 6);
    assert_eq!(team_names(&copy).await, ["Alpha", "Bravo", "Charlie"]);

    // A restart with nothing pending writes no further backup.
    pool.close().await;
    let pool = open_db(&db.url(), &config, &migrator).await.unwrap();
    assert_eq!(files(&config.dir).len(), 1);
    pool.close().await;
}

#[tokio::test]
async fn no_migration_without_the_backup() {
    let db = test_db().await;
    db.pool.close().await;
    let migrator = migrator_with_dummy(db.dir.path()).await;
    let config = db.config();
    // A regular file where the backup directory should be.
    std::fs::write(&config.dir, b"x").unwrap();

    let err = open_db(&db.url(), &config, &migrator).await.unwrap_err();
    assert!(matches!(err, OpenError::Backup { version: 6, .. }), "{err}");
    assert!(err.to_string().contains("not migrated"), "{err}");

    // The database is untouched: still schema v6, data intact.
    let pool = store::connect(&db.url()).await.unwrap();
    assert!(!table_exists(&pool, DUMMY).await);
    assert_eq!(applied_version(&pool).await, 6);
    assert_eq!(team_names(&pool).await, ["Alpha", "Bravo", "Charlie"]);
    pool.close().await;
}

#[tokio::test]
async fn a_fresh_database_needs_no_backup() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("new.db");
    let config = backup::BackupConfig::new(dir.path().join("backups"));

    let pool = open_db(&common::url(&path), &config, store::migrator())
        .await
        .unwrap();
    assert!(table_exists(&pool, "teams").await);
    assert!(files(&config.dir).is_empty());
    pool.close().await;
}

#[tokio::test]
async fn nothing_pending_means_no_backup() {
    let db = test_db().await;
    db.pool.close().await;
    let config = db.config();
    let pool = open_db(&db.url(), &config, store::migrator())
        .await
        .unwrap();
    assert!(files(&config.dir).is_empty());
    pool.close().await;
}
