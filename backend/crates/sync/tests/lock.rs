//! The lock ticker writes while the reconciler writes (T06). SQLite has one
//! writer at a time; neither may fail because of the other.

mod common;

use common::{SIM, env, scenario};
use domain::Clock;
use sources::SourceAdapter;

#[tokio::test]
async fn lock_ticker_and_reconciler_do_not_fail_each_other() {
    let env = env(vec![scenario("e", &[60, 60, 120, 120, 180, 180])]).await;
    env.follow_quietly("e").await;
    env.catch_up(&["e"]).await;
    let f = env.source.fixture("e");

    let pool = env.pool.clone();
    let clock = env.clock.clone();
    let ticker = tokio::spawn(async move {
        for _ in 0..300 {
            sync::lock::lock_due(&pool, clock.now()).await.unwrap();
            // Unlock again, so every round writes.
            sqlx::query("UPDATE matches SET locked = 0")
                .execute(&pool)
                .await
                .unwrap();
            tokio::task::yield_now().await;
        }
    });
    let mut failed = 0;
    for minute in 0..40 {
        let now = env.set_minute(minute * 5);
        let ms = f.fetch_event_matches("e").await.unwrap();
        failed += env
            .reconciler
            .ingest_matches(SIM, &ms, now)
            .await
            .unwrap()
            .failed;
    }
    ticker.await.unwrap();
    assert_eq!(
        failed, 0,
        "matches failed to reconcile while the ticker ran"
    );
}
