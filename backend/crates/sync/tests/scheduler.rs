//! The sync scheduler on a fake clock (T06 acceptance criteria 1 and 2):
//! a simulated busy day stays under budget, and a match's poll cadence
//! follows its state from upcoming to live to finished.

mod common;

use chrono::{DateTime, Duration, Utc};
use common::{BUDGET, Call, Env, MATCH_MINUTES, SIM, day, env, max_in_window, polls_of, scenario};
use domain::{EntityType, MatchId, MatchStatus};
use sources::CallKind;
use sync::{BudgetLimits, SyncConfig, SyncHandle};

/// Busy day: 3 followed events, 20 matches, at most 4 live at once.
fn busy_day() -> Vec<sources::fixture::Scenario> {
    let h = |h: i64| h * 60;
    vec![
        scenario(
            "a",
            &[h(10), h(10), h(13), h(13), h(16), h(16), h(19), h(19)],
        ),
        scenario("b", &[h(7), h(11), h(14), h(17), h(20), h(22)]),
        scenario(
            "c",
            &[
                h(8),
                h(9),
                11 * 60 + 30,
                14 * 60 + 30,
                17 * 60 + 30,
                20 * 60 + 30,
            ],
        ),
    ]
}

/// Typical day: one followed event with 6 matches, two at a time.
fn typical_day() -> Vec<sources::fixture::Scenario> {
    vec![scenario(
        "t",
        &[14 * 60, 14 * 60, 17 * 60, 17 * 60, 20 * 60, 20 * 60],
    )]
}

/// Most matches live at the same minute.
fn max_live(scenarios: &[sources::fixture::Scenario]) -> usize {
    let starts: Vec<i64> = scenarios
        .iter()
        .flat_map(|s| s.matches.iter().map(|m| m.start))
        .collect();
    (0..24 * 60)
        .map(|t| {
            starts
                .iter()
                .filter(|s| **s <= t && t < **s + MATCH_MINUTES)
                .count()
        })
        .max()
        .unwrap()
}

struct DayResult {
    env: Env,
    handle: SyncHandle,
    calls: Vec<Call>,
    /// When each match was locked by the lock ticker.
    locked_at: Vec<(MatchId, DateTime<Utc>)>,
    deferred: u32,
}

/// Follows every event at midnight, then ticks every simulated minute for
/// `minutes`, running the lock ticker alongside.
async fn simulate(scenarios: Vec<sources::fixture::Scenario>, minutes: i64) -> DayResult {
    let ids: Vec<String> = scenarios.iter().map(|s| s.event.id.clone()).collect();
    let env = env(scenarios).await;
    let (mut scheduler, handle) = env.scheduler(Some(BUDGET)).await;
    for id in &ids {
        env.follow(&handle, id).await;
    }
    let mut locked_at = Vec::new();
    let mut deferred = 0;
    for minute in 0..=minutes {
        let now = env.set_minute(minute);
        let rep = scheduler.tick().await.unwrap();
        assert_eq!(rep.failed, 0, "minute {minute}");
        assert_eq!(rep.reconciled.failed, 0, "minute {minute}");
        deferred += rep.deferred;
        for id in sync::lock::lock_due(&env.pool, now).await.unwrap() {
            locked_at.push((id, now));
        }
    }
    let calls = env.source.calls();
    DayResult {
        env,
        handle,
        calls,
        locked_at,
        deferred,
    }
}

fn total(calls: &[Call], until: DateTime<Utc>) -> u32 {
    calls.iter().filter(|c| c.at < until).map(|c| c.cost).sum()
}

fn report(label: &str, r: &DayResult) {
    let day_end = day() + Duration::days(1);
    let by_kind = |k: CallKind| -> u32 {
        r.calls
            .iter()
            .filter(|c| c.kind == k && c.at < day_end)
            .map(|c| c.cost)
            .sum()
    };
    // Printed for the README table (`cargo test -p sync --test scheduler -- --nocapture`).
    println!(
        "{label}: {} requests in 24 h (event {}, event matches {}, matches {}, live {}); \
         peak {} per hour, {} per minute",
        total(&r.calls, day_end),
        by_kind(CallKind::FetchEvent),
        by_kind(CallKind::FetchEventMatches),
        by_kind(CallKind::FetchMatches),
        by_kind(CallKind::FetchLive),
        max_in_window(&r.calls, Duration::hours(1)),
        max_in_window(&r.calls, Duration::minutes(1)),
    );
}

async fn match_id(env: &Env, ext: &str) -> MatchId {
    MatchId(
        store::refs::resolve(&env.pool, SIM, EntityType::Match, ext)
            .await
            .unwrap()
            .unwrap(),
    )
}

#[tokio::test]
async fn busy_day_stays_under_budget() {
    let scenarios = busy_day();
    let n: usize = scenarios.iter().map(|s| s.matches.len()).sum();
    assert_eq!((scenarios.len(), n, max_live(&scenarios)), (3, 20, 4));

    // Until 03:00 the next day, so the last corrections are in.
    let r = simulate(scenarios, 27 * 60).await;
    report("busy day", &r);

    assert!(max_in_window(&r.calls, Duration::hours(1)) <= BUDGET.per_hour);
    assert!(max_in_window(&r.calls, Duration::minutes(1)) <= BUDGET.per_minute);
    assert_eq!(r.deferred, 0, "the budget was never short");
    let day_total = total(&r.calls, day() + Duration::days(1));
    assert!(day_total < 1000, "{day_total} requests");

    // Every match was followed to its end and locked at its start.
    let finished: i64 =
        sqlx::query_scalar("SELECT count(*) FROM matches WHERE status = 'finished' AND locked = 1")
            .fetch_one(&r.env.pool)
            .await
            .unwrap();
    assert_eq!(finished, 20);
    for (id, at) in &r.locked_at {
        let m = store::matches::get(&r.env.pool, *id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(Some(*at), m.fields.scheduled_at, "match {id:?}");
    }
    assert_eq!(r.locked_at.len(), 20);

    // The counters agree with what the source saw.
    let status = r.handle.status().await.unwrap();
    let src = &status.sources[0];
    assert_eq!(src.source, SIM);
    assert_eq!(src.budget_per_hour, Some(500));
    let hour = src.hour_started_at;
    let in_hour: u32 = r
        .calls
        .iter()
        .filter(|c| c.at >= hour && c.at < hour + Duration::hours(1))
        .map(|c| c.cost)
        .sum();
    assert_eq!(src.requests_this_hour, i64::from(in_hour));
    assert!(status.jobs.iter().any(|j| j.job == "live"));
    assert!(
        status
            .jobs
            .iter()
            .all(|j| j.last_error.is_none() && j.last_success_at.is_some()),
        "{:?}",
        status.jobs
    );
}

#[tokio::test]
async fn typical_day_costs_less() {
    let r = simulate(typical_day(), 27 * 60).await;
    report("typical day", &r);
    let day_total = total(&r.calls, day() + Duration::days(1));
    assert!(day_total < 500, "{day_total} requests");
    assert_eq!(r.deferred, 0);
}

/// Gaps between consecutive polls whose both ends lie in `[from, to)`.
fn gaps_within(polls: &[DateTime<Utc>], from: DateTime<Utc>, to: DateTime<Utc>) -> Vec<i64> {
    polls
        .windows(2)
        .filter(|w| w[0] >= from && w[1] < to)
        .map(|w| (w[1] - w[0]).num_minutes())
        .collect()
}

#[tokio::test]
async fn cadence_follows_the_match_from_upcoming_to_finished() {
    // Match "a-m2" starts at 13:00 and ends at 15:15.
    let r = simulate(busy_day(), 27 * 60).await;
    let mut polls = polls_of(&r.calls, "a-m2");
    polls.dedup();
    let start = day() + Duration::hours(13);
    let end = start + Duration::minutes(MATCH_MINUTES);
    let m = store::matches::get(&r.env.pool, match_id(&r.env, "a-m2").await)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(m.fields.status, MatchStatus::Finished);
    assert_eq!(m.fields.ended_at, Some(end));

    let upcoming = gaps_within(&polls, day(), start - Duration::hours(1));
    assert!(upcoming.len() >= 20, "{upcoming:?}");
    assert!(upcoming.iter().all(|g| *g == 30), "{upcoming:?}");

    let imminent = gaps_within(&polls, start - Duration::hours(1), start);
    assert!(imminent.len() >= 10, "{imminent:?}");
    assert!(imminent.iter().all(|g| *g == 5), "{imminent:?}");

    // Live from the first poll after the start until the end.
    let live = gaps_within(&polls, start + Duration::minutes(7), end);
    assert!(live.len() >= 50, "{live:?}");
    assert!(live.iter().all(|g| *g == 2), "{live:?}");

    // After the poll that saw it finished: +10 min, +2 h, then nothing.
    let after: Vec<DateTime<Utc>> = polls
        .iter()
        .copied()
        .filter(|t| *t > end + Duration::minutes(2))
        .collect();
    assert_eq!(
        after,
        vec![end + Duration::minutes(10), end + Duration::hours(2)]
    );

    // The event structure is refreshed every 6 h while the event runs.
    let structure: Vec<i64> = r
        .calls
        .iter()
        .filter(|c| c.kind == CallKind::FetchEvent && c.ids == ["a"])
        .map(|c| (c.at - day()).num_minutes())
        .collect();
    assert_eq!(&structure[..4], &[0, 360, 720, 1080]);
}

#[tokio::test]
async fn a_short_budget_makes_lower_priorities_wait() {
    // At 11:45 four matches are live and the DB is up to date. Nothing has
    // run yet, so every job is due.
    let env = env(busy_day()).await;
    env.set_minute(11 * 60 + 45);
    for e in ["a", "b", "c"] {
        env.follow_quietly(e).await;
    }
    env.catch_up(&["a", "b", "c"]).await;
    let config = SyncConfig {
        startup_jitter: Duration::zero(),
        ..SyncConfig::default()
    };
    // 5 per hour: live, corrections and upcoming (1 each) fit, but the
    // structure jobs (4 each) must keep 20% back and wait.
    let tiny = BudgetLimits {
        per_hour: 5,
        per_minute: 30,
    };
    let (mut s, _handle) = env.scheduler_with(Some(tiny), config).await;
    let rep = s.tick().await.unwrap();
    let kinds: Vec<CallKind> = env.source.calls().iter().map(|c| c.kind).collect();
    assert_eq!(kinds.first(), Some(&CallKind::FetchLive), "{kinds:?}");
    assert!(!kinds.contains(&CallKind::FetchEvent), "{kinds:?}");
    assert_eq!(rep.deferred, 3, "{rep:?}");
    assert_eq!(rep.ran, 3, "{rep:?}");
}

#[tokio::test]
async fn search_is_cached_and_skipped_when_the_budget_is_short() {
    let env = env(busy_day()).await;
    let searches = |env: &Env| {
        env.source
            .calls()
            .iter()
            .filter(|c| c.kind == CallKind::SearchEvents)
            .count()
    };
    let (_s, handle) = env.scheduler(Some(BUDGET)).await;
    let first = handle.search("Event A").await;
    assert_eq!(first.hits.len(), 1);
    assert_eq!(first.hits[0].event.dto.ext_id, "a");
    let again = handle.search("  event   a ").await;
    assert_eq!(again, first);
    assert_eq!(searches(&env), 1, "the repeat came from the cache");

    // After 24 h the cache expires.
    env.set_minute(24 * 60 + 1);
    handle.search("event a").await;
    assert_eq!(searches(&env), 2);

    // Search keeps 30% of the hourly budget back: of 10 requests per hour
    // it may use 7.
    let (_s, handle) = env
        .scheduler(Some(BudgetLimits {
            per_hour: 10,
            per_minute: 30,
        }))
        .await;
    for i in 0..7 {
        let out = handle.search(&format!("query {i}")).await;
        assert!(out.skipped.is_empty(), "{i}: {out:?}");
    }
    let out = handle.search("query 7").await;
    assert!(out.hits.is_empty());
    assert_eq!(out.skipped.len(), 1, "{out:?}");
    assert_eq!(searches(&env), 2 + 7);
}

#[tokio::test]
async fn run_loop_wakes_on_request_and_stops_on_cancel() {
    use std::time::Duration as StdDuration;
    use tokio_util::sync::CancellationToken;

    let env = env(typical_day()).await;
    // Ticks are an hour apart in real time: only the request wakes it.
    let config = SyncConfig {
        tick_every: Duration::hours(1),
        ..SyncConfig::default()
    };
    let (s, handle) = env.scheduler_with(Some(BUDGET), config).await;
    let cancel = CancellationToken::new();
    let scheduler = tokio::spawn(s.run(cancel.clone()));
    let ticker = tokio::spawn(sync::run_lock_ticker(
        env.pool.clone(),
        env.clock.clone(),
        StdDuration::from_millis(10),
        cancel.clone(),
    ));
    tokio::time::sleep(StdDuration::from_millis(50)).await;
    assert!(env.source.calls().is_empty(), "nothing followed yet");

    env.follow(&handle, "t").await;
    tokio::time::timeout(StdDuration::from_secs(5), async {
        while !env
            .source
            .calls()
            .iter()
            .any(|c| c.kind == CallKind::FetchEventMatches)
        {
            tokio::time::sleep(StdDuration::from_millis(10)).await;
        }
    })
    .await
    .expect("the follow woke the scheduler");

    // The lock ticker runs on the same (fake) clock.
    env.set_minute(14 * 60);
    tokio::time::timeout(StdDuration::from_secs(5), async {
        while env.count("matches WHERE locked = 1").await < 2 {
            tokio::time::sleep(StdDuration::from_millis(10)).await;
        }
    })
    .await
    .expect("the 14:00 matches were locked");

    cancel.cancel();
    for task in [scheduler, ticker] {
        tokio::time::timeout(StdDuration::from_secs(1), task)
            .await
            .expect("stopped on cancel")
            .unwrap();
    }
}
