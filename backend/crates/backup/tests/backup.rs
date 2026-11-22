//! Daily and manual backups: written, checked, copied, pruned, reported.

mod common;

use std::time::Duration;

use backup::{BackupError, BackupKind, BackupService, Policy, name};
use chrono::{TimeZone, Utc};
use common::{files, open_backup, team_names, test_db};
use tokio_util::sync::CancellationToken;

#[tokio::test]
async fn a_backup_is_written_checked_and_readable() {
    let db = test_db().await;
    let service = BackupService::new(db.pool.clone(), db.config());

    let out = service.run(BackupKind::Manual).await.unwrap();
    assert_eq!(out.backup.kind, BackupKind::Manual);
    assert!(out.backup.size > 0);
    assert!(out.extra_error.is_none());
    assert_eq!(
        files(&db.config().dir),
        vec![out.backup.file_name.clone()],
        "only the finished file, no temp files"
    );
    assert_eq!(
        name::parse(&out.backup.file_name).map(|(_, k)| k),
        Some(BackupKind::Manual)
    );

    let copy = open_backup(&out.backup.path, db.dir.path()).await;
    assert_eq!(team_names(&copy).await, ["Alpha", "Bravo", "Charlie"]);
    let (check,): (String,) = sqlx::query_as("PRAGMA integrity_check")
        .fetch_one(&copy)
        .await
        .unwrap();
    assert_eq!(check, "ok");

    let status = service.status().await;
    assert_eq!(status.last.as_ref(), Some(&out.backup));
    assert_eq!(status.last_error, None);
    assert!(!status.running);
    assert!(status.next_run.is_some());
}

#[tokio::test]
async fn the_extra_dir_gets_a_copy() {
    let db = test_db().await;
    let mut config = db.config();
    let extra = db.dir.path().join("nas/pickem");
    config.extra_dir = Some(extra.clone());
    let service = BackupService::new(db.pool.clone(), config);

    let out = service.run(BackupKind::Daily).await.unwrap();
    assert!(out.extra_error.is_none());
    assert_eq!(files(&extra), vec![out.backup.file_name.clone()]);
    assert_eq!(
        std::fs::read(extra.join(&out.backup.file_name)).unwrap(),
        std::fs::read(&out.backup.path).unwrap()
    );
}

#[tokio::test]
async fn a_broken_extra_dir_is_reported_but_the_backup_counts() {
    let db = test_db().await;
    let mut config = db.config();
    // A regular file where the directory should be.
    let extra = db.dir.path().join("not-a-dir");
    std::fs::write(&extra, b"x").unwrap();
    config.extra_dir = Some(extra);
    let service = BackupService::new(db.pool.clone(), config);

    let out = service.run(BackupKind::Daily).await.unwrap();
    assert!(
        out.extra_error
            .as_deref()
            .unwrap()
            .contains("BACKUP_EXTRA_DIR")
    );
    assert!(out.backup.path.exists());
    let status = service.status().await;
    assert_eq!(status.last.as_ref(), Some(&out.backup));
    assert!(
        status
            .last_error
            .unwrap()
            .message
            .contains("BACKUP_EXTRA_DIR")
    );
}

#[tokio::test]
async fn old_backups_are_pruned_in_both_directories() {
    let db = test_db().await;
    let mut config = db.config();
    let extra = db.dir.path().join("extra");
    config.extra_dir = Some(extra.clone());
    config.policy = Policy {
        keep_daily: 3,
        keep_weekly: 0,
        keep_special: 1,
    };
    // Old backups on five earlier days and two pre-migration ones. Retention
    // reads only the names, so empty files do.
    let mut old = Vec::new();
    for day in 1..=5 {
        let at = Utc.with_ymd_and_hms(2026, 1, day, 4, 0, 0).unwrap();
        old.push(name::format(at, BackupKind::Daily));
    }
    for v in [5, 6] {
        let at = Utc.with_ymd_and_hms(2026, 1, v as u32, 5, 0, 0).unwrap();
        old.push(name::format(at, BackupKind::PreMigrate { version: v }));
    }
    for dir in [&config.dir, &extra] {
        std::fs::create_dir_all(dir).unwrap();
        for f in &old {
            std::fs::write(dir.join(f), b"").unwrap();
        }
        std::fs::write(dir.join("notes.txt"), b"not a backup").unwrap();
    }
    let service = BackupService::new(db.pool.clone(), config.clone());

    let out = service.run(BackupKind::Manual).await.unwrap();
    // Per directory, kept: the 3 newest days (today, Jan 5, Jan 4), the
    // newest pre-migration backup (v6) and the foreign file. Deleted: the
    // dailies of Jan 1 to 3 and the v5 backup.
    let expected = {
        let mut v = vec![
            old[3].clone(),
            old[4].clone(),
            old[6].clone(),
            out.backup.file_name.clone(),
            "notes.txt".to_owned(),
        ];
        v.sort();
        v
    };
    assert_eq!(files(&config.dir), expected);
    assert_eq!(files(&extra), expected);
    assert_eq!(out.pruned, 2 * 4);
}

#[tokio::test]
async fn only_one_backup_runs_at_a_time() {
    let db = test_db().await;
    let service = BackupService::new(db.pool.clone(), db.config());
    let (a, b) = tokio::join!(
        service.run(BackupKind::Manual),
        service.run(BackupKind::Manual)
    );
    let refused = [&a, &b]
        .iter()
        .filter(|r| matches!(r, Err(BackupError::AlreadyRunning)))
        .count();
    assert_eq!(refused, 1, "{a:?} / {b:?}");
    assert!(a.is_ok() || b.is_ok());
}

#[tokio::test]
async fn a_failure_is_reported_and_cleared_by_the_next_success() {
    let db = test_db().await;
    let config = db.config();
    // A regular file where the backup directory should be.
    std::fs::write(&config.dir, b"x").unwrap();
    let service = BackupService::new(db.pool.clone(), config.clone());

    let err = service.run(BackupKind::Manual).await.unwrap_err();
    assert!(matches!(err, BackupError::Io { .. }), "{err}");
    let status = service.status().await;
    assert!(status.last.is_none());
    assert!(status.last_error.is_some());

    std::fs::remove_file(&config.dir).unwrap();
    service.run(BackupKind::Manual).await.unwrap();
    assert_eq!(service.status().await.last_error, None);
}

#[tokio::test]
async fn the_schedule_catches_up_and_stops() {
    let db = test_db().await;
    let config = db.config();
    // A leftover from an interrupted run is cleaned up.
    std::fs::create_dir_all(&config.dir).unwrap();
    std::fs::write(
        config.dir.join("pickem-20260101T040000Z-daily.db.zst.tmp"),
        b"",
    )
    .unwrap();
    let service = BackupService::new(db.pool.clone(), config.clone());
    let stop = CancellationToken::new();
    let task = tokio::spawn(service.clone().run_schedule(stop.clone()));

    // No backup since the last 04:00 slot: one is taken at once.
    let mut found = Vec::new();
    for _ in 0..100 {
        found = files(&config.dir);
        if found.len() == 1 && found[0].ends_with("-daily.db.zst") {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_eq!(found.len(), 1, "{found:?}");
    assert!(found[0].ends_with("-daily.db.zst"), "{found:?}");

    stop.cancel();
    tokio::time::timeout(Duration::from_secs(5), task)
        .await
        .expect("the schedule stops on cancel")
        .unwrap();
    assert_eq!(files(&config.dir).len(), 1, "no second backup");
}

#[tokio::test]
async fn backup_time_off_disables_the_schedule() {
    let db = test_db().await;
    let mut config = db.config();
    config.time = None;
    let service = BackupService::new(db.pool.clone(), config.clone());
    tokio::time::timeout(
        Duration::from_secs(5),
        service.clone().run_schedule(CancellationToken::new()),
    )
    .await
    .expect("returns at once");
    assert!(files(&config.dir).is_empty());
    assert_eq!(service.status().await.next_run, None);
}
