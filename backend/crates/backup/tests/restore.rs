//! `app restore`: back up, change data, restore, and the data matches.

mod common;

use backup::{BackupKind, BackupService, restore};
use common::{add_team, files, team_names, test_db, url};

#[tokio::test]
async fn round_trip_back_up_change_restore() {
    let db = test_db().await;
    let config = db.config();
    let service = BackupService::new(db.pool.clone(), config.clone());
    let snapshot = service.run(BackupKind::Manual).await.unwrap().backup;

    // Change the data after the backup.
    add_team(&db.pool, "Delta").await;
    sqlx::query("DELETE FROM teams WHERE name = 'Alpha'")
        .execute(&db.pool)
        .await
        .unwrap();
    assert_eq!(team_names(&db.pool).await, ["Bravo", "Charlie", "Delta"]);
    // As documented: stop the app first.
    db.pool.close().await;

    let report = restore(&snapshot.path, &db.path, &config).await.unwrap();
    assert_eq!(report.db_path, db.path);
    assert!(report.moved_aside.is_none());
    let safety = report.safety_backup.expect("the replaced database is kept");
    assert_eq!(safety.kind, BackupKind::PreRestore);
    // No stale WAL or temp file next to the restored database.
    let wal = db.path.with_file_name("pickem.db-wal");
    assert!(!wal.exists());
    assert!(!db.path.with_file_name("pickem.db.restore-tmp").exists());

    let pool = store::connect(&db.url()).await.unwrap();
    assert_eq!(team_names(&pool).await, ["Alpha", "Bravo", "Charlie"]);
    pool.close().await;

    // The safety backup undoes the restore.
    restore(&safety.path, &db.path, &config).await.unwrap();
    let pool = store::connect(&db.url()).await.unwrap();
    assert_eq!(team_names(&pool).await, ["Bravo", "Charlie", "Delta"]);
    pool.close().await;
}

#[tokio::test]
async fn a_corrupt_backup_changes_nothing() {
    let db = test_db().await;
    let config = db.config();
    db.pool.close().await;
    let bad = db.dir.path().join("pickem-20261122T030000Z-manual.db.zst");
    std::fs::write(&bad, b"definitely not zstd").unwrap();
    let not_sqlite = db.dir.path().join("plain.db");
    std::fs::write(&not_sqlite, vec![7u8; 8192]).unwrap();

    for file in [&bad, &not_sqlite] {
        let err = restore(file, &db.path, &config).await.unwrap_err();
        assert!(err.to_string().contains("nothing was changed"), "{err}");
    }
    assert!(!db.path.with_file_name("pickem.db.restore-tmp").exists());
    assert!(files(&config.dir).is_empty(), "no safety backup either");
    let pool = store::connect(&db.url()).await.unwrap();
    assert_eq!(team_names(&pool).await, ["Alpha", "Bravo", "Charlie"]);
    pool.close().await;
}

#[tokio::test]
async fn a_broken_current_database_is_moved_aside() {
    let db = test_db().await;
    let config = db.config();
    let service = BackupService::new(db.pool.clone(), config.clone());
    let snapshot = service.run(BackupKind::Manual).await.unwrap().backup;
    db.pool.close().await;
    std::fs::write(&db.path, vec![0xAB; 16384]).unwrap();

    let report = restore(&snapshot.path, &db.path, &config).await.unwrap();
    assert!(report.safety_backup.is_none());
    let aside = report.moved_aside.expect("moved aside");
    assert_eq!(std::fs::read(&aside).unwrap(), vec![0xAB; 16384]);

    let pool = store::connect(&url(&db.path)).await.unwrap();
    assert_eq!(team_names(&pool).await, ["Alpha", "Bravo", "Charlie"]);
    pool.close().await;
}

#[tokio::test]
async fn restoring_into_an_empty_data_dir_works() {
    let db = test_db().await;
    let config = db.config();
    let service = BackupService::new(db.pool.clone(), config.clone());
    let snapshot = service.run(BackupKind::Manual).await.unwrap().backup;
    db.pool.close().await;

    let target = db.dir.path().join("fresh/pickem.db");
    let report = restore(&snapshot.path, &target, &config).await.unwrap();
    assert!(report.safety_backup.is_none() && report.moved_aside.is_none());
    let pool = store::connect(&url(&target)).await.unwrap();
    assert_eq!(team_names(&pool).await, ["Alpha", "Bravo", "Charlie"]);
    pool.close().await;
}
