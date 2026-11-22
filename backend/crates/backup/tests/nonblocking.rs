//! A backup never blocks requests: writes and reads keep answering within
//! 1 s while a backup of a large database runs.
//!
//! The normal suite uses 10 MB. The 50 MB case of the acceptance criterion
//! is ignored by default; run it on the target machine with
//! `cargo test --release -p backup --test nonblocking -- --ignored --nocapture`.

mod common;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use backup::{BackupKind, BackupService};
use common::test_db;

/// Grows the database by about `mb` MB of moderately compressible rows.
async fn fill(pool: &store::SqlitePool, mb: i64) {
    sqlx::query("CREATE TABLE filler (id INTEGER PRIMARY KEY, data TEXT NOT NULL) STRICT")
        .execute(pool)
        .await
        .unwrap();
    // 1 KiB per row: the hex of 512 random bytes.
    sqlx::query(
        "INSERT INTO filler (data) \
         WITH RECURSIVE c(x) AS (SELECT 1 UNION ALL SELECT x + 1 FROM c WHERE x < ?) \
         SELECT hex(randomblob(512)) FROM c",
    )
    .bind(mb * 1024)
    .execute(pool)
    .await
    .unwrap();
}

struct Measured {
    backup: Duration,
    max_request: Duration,
    requests: usize,
}

async fn backup_under_load(mb: i64) -> Measured {
    let db = test_db().await;
    fill(&db.pool, mb).await;
    let service = BackupService::new(db.pool.clone(), db.config());

    let stop = Arc::new(AtomicBool::new(false));
    let load = {
        let pool = db.pool.clone();
        let stop = stop.clone();
        tokio::spawn(async move {
            let mut max = Duration::ZERO;
            let mut n = 0usize;
            while !stop.load(Ordering::Relaxed) {
                let t = Instant::now();
                // A write like a pick, then a read like a page load.
                let mut tx = store::begin_write(&pool).await.unwrap();
                sqlx::query("INSERT INTO teams (name) VALUES ('load')")
                    .execute(&mut *tx)
                    .await
                    .unwrap();
                tx.commit().await.unwrap();
                let _: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM teams")
                    .fetch_one(&pool)
                    .await
                    .unwrap();
                max = max.max(t.elapsed());
                n += 1;
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
            (max, n)
        })
    };
    // Let the load start before the backup.
    tokio::time::sleep(Duration::from_millis(50)).await;
    let out = service.run(BackupKind::Manual).await.unwrap();
    stop.store(true, Ordering::Relaxed);
    let (max_request, requests) = load.await.unwrap();
    println!(
        "{mb} MB: backup {:?} ({} bytes compressed), {requests} requests, slowest {max_request:?}",
        out.duration, out.backup.size
    );
    Measured {
        backup: out.duration,
        max_request,
        requests,
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn requests_are_not_blocked_during_a_backup() {
    let m = backup_under_load(10).await;
    assert!(m.requests > 0);
    assert!(
        m.max_request < Duration::from_secs(1),
        "slowest request {:?}",
        m.max_request
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "50 MB timing run; use --release on the target machine"]
async fn fifty_mb_backup_is_fast_and_never_blocks() {
    let m = backup_under_load(50).await;
    assert!(
        m.backup < Duration::from_secs(30),
        "backup took {:?}",
        m.backup
    );
    assert!(
        m.max_request < Duration::from_secs(1),
        "slowest request {:?}",
        m.max_request
    );
}
