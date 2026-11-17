//! Migrations run on an empty database and are idempotent on restart.

mod common;

use common::{at, team, test_db};

const TABLES: [&str; 19] = [
    "audit_log",
    "events",
    "external_refs",
    "field_sources",
    "followed_events",
    "guild_members",
    "guild_settings",
    "maps",
    "matches",
    "pick_maps",
    "pick_results",
    "pick_revisions",
    "picks",
    "source_records",
    "stages",
    "sync_state",
    "teams",
    "unlinked_refs",
    "users",
];

async fn table_names(pool: &store::SqlitePool) -> Vec<String> {
    sqlx::query_scalar(
        "SELECT name FROM sqlite_schema WHERE type = 'table' \
         AND name NOT LIKE 'sqlite_%' AND name <> '_sqlx_migrations' ORDER BY name",
    )
    .fetch_all(pool)
    .await
    .unwrap()
}

#[tokio::test]
async fn empty_database_gets_every_table() {
    let db = test_db().await;
    assert_eq!(table_names(&db.pool).await, TABLES);

    // Every table is STRICT.
    let strict: Vec<(String, i64)> =
        sqlx::query_as("SELECT name, strict FROM pragma_table_list WHERE schema = 'main'")
            .fetch_all(&db.pool)
            .await
            .unwrap();
    for (name, is_strict) in strict {
        if TABLES.contains(&name.as_str()) {
            assert_eq!(is_strict, 1, "{name} is not STRICT");
        }
    }

    let (check,): (String,) = sqlx::query_as("PRAGMA integrity_check")
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_eq!(check, "ok");
}

#[tokio::test]
async fn migrations_are_idempotent_on_restart() {
    let db = test_db().await;
    let id = team(&db.pool, "Alpha").await;

    // A second run on the same pool is a no-op.
    store::migrate(&db.pool).await.unwrap();
    db.pool.close().await;

    // So is a restart: open again (connect + migrate) on the same file.
    let pool = store::open(&db.url()).await.unwrap();
    assert_eq!(table_names(&pool).await, TABLES);
    let applied: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM _sqlx_migrations")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(applied, 3, "0001 to 0003");
    let kept = store::teams::get(&pool, id).await.unwrap().unwrap();
    assert_eq!(kept.name, "Alpha");
    pool.close().await;
}

#[tokio::test]
async fn pragmas_hold_on_every_pooled_connection() {
    let db = test_db().await;
    // Hold several connections at once, so the pool must open more than one.
    let mut conns = Vec::new();
    for _ in 0..3 {
        conns.push(db.pool.acquire().await.unwrap());
    }
    for conn in &mut conns {
        let (mode,): (String,) = sqlx::query_as("PRAGMA journal_mode")
            .fetch_one(&mut **conn)
            .await
            .unwrap();
        let (fk,): (i64,) = sqlx::query_as("PRAGMA foreign_keys")
            .fetch_one(&mut **conn)
            .await
            .unwrap();
        let (timeout,): (i64,) = sqlx::query_as("PRAGMA busy_timeout")
            .fetch_one(&mut **conn)
            .await
            .unwrap();
        assert_eq!((mode.as_str(), fk, timeout), ("wal", 1, 5000));
    }
}

#[tokio::test]
async fn timestamps_use_the_fixed_utc_format() {
    let db = test_db().await;
    let e = common::event(&db.pool, "Major").await;
    let stored: String = sqlx::query_scalar("SELECT begin_at FROM events WHERE id = ?")
        .bind(e.0)
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_eq!(stored, "2026-11-20T10:00:00.000Z");
    assert_eq!(stored, store::format_ts(&at(10, 0)));

    // Any other spelling of a time is rejected, e.g. sqlx's own encoding.
    common::assert_rejected(
        common::exec(
            &db.pool,
            "INSERT INTO events (name, status, begin_at) \
             VALUES ('x', 'upcoming', '2026-11-20T10:00:00+00:00')",
        )
        .await,
        "CHECK constraint failed",
    );
}
