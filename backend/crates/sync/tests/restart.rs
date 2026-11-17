//! Restart safety (T06 acceptance criterion 3): killing the process
//! mid-sync and restarting causes no duplicate data and no burst of
//! requests.

mod common;

use chrono::Duration;
use common::{BUDGET, Env, SIM, day, env, reconciler, scenario};
use domain::Clock;
use sources::{CallKind, SourceAdapter};

fn scenarios() -> Vec<sources::fixture::Scenario> {
    // Six matches in the afternoon: all within 48 h, none within 1 h.
    vec![scenario(
        "e",
        &[14 * 60, 14 * 60, 17 * 60, 17 * 60, 20 * 60, 20 * 60],
    )]
}

/// Events, matches, teams, external refs (event, stage, matches, teams) and
/// source records (stages live inside the event record).
async fn counts(env: &Env) -> [i64; 5] {
    [
        env.count("events").await,
        env.count("matches").await,
        env.count("teams").await,
        env.count("external_refs").await,
        env.count("source_records").await,
    ]
}

#[tokio::test]
async fn killed_mid_sync_restarts_without_duplicates_or_burst() {
    let mut env = env(scenarios()).await;
    let (mut s, handle) = env.scheduler(Some(BUDGET)).await;
    let event = env.follow(&handle, "e").await;

    // The structure sync fetches the event (call 0), then hangs on the
    // match list (call 1): the process dies there.
    let reached = env.source.hang_on(1);
    let task = tokio::spawn(async move { s.tick().await });
    reached.notified().await;
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());

    // Also as if the kill hit in the middle of a batch: half the matches
    // made it into the DB.
    let ms = env
        .source
        .fixture("e")
        .fetch_event_matches("e")
        .await
        .unwrap();
    env.reconciler
        .ingest_matches(SIM, &ms[..3], env.clock.now())
        .await
        .unwrap();
    assert_eq!(env.count("matches").await, 3);

    // Restart: a new reconciler and scheduler on the same database.
    env.reconciler = reconciler(&env.pool);
    let before = env.source.calls().len();
    let (mut s, handle) = env.scheduler(Some(BUDGET)).await;
    let status = handle.status().await.unwrap();
    assert_eq!(
        status.sources[0].budget_left,
        Some(BUDGET.per_hour - 4),
        "the budget is restored, not refilled"
    );
    let job = status
        .jobs
        .iter()
        .find(|j| j.job == format!("structure:{}", event.0))
        .unwrap();
    assert!(job.last_run_at.is_some() && job.last_success_at.is_none());

    // Ten minutes after the restart: each job ran once, the interrupted
    // structure sync completed, nothing is duplicated.
    for minute in 0..=10 {
        env.set_minute(minute);
        let rep = s.tick().await.unwrap();
        assert_eq!(rep.failed, 0);
    }
    let mut kinds: Vec<String> = env.source.calls()[before..]
        .iter()
        .map(|c| format!("{:?}", c.kind))
        .collect();
    kinds.sort();
    assert_eq!(
        kinds,
        ["FetchEvent", "FetchEventMatches", "FetchMatches"],
        "the structure job and the upcoming job, once each"
    );
    assert_eq!(counts(&env).await, [1, 6, 12, 1 + 1 + 6 + 12, 1 + 6 + 12]);

    // A second restart 20 minutes later: nothing is due yet, so nothing is
    // sent until each job's cadence is up.
    let before = env.source.calls().len();
    let (mut s, _handle) = env.scheduler(Some(BUDGET)).await;
    for minute in 20..=45 {
        env.set_minute(minute);
        s.tick().await.unwrap();
    }
    let calls = env.source.calls()[before..].to_vec();
    let times: Vec<i64> = calls.iter().map(|c| (c.at - day()).num_minutes()).collect();
    let upcoming_first = env.source.calls()[..before]
        .iter()
        .rev()
        .find(|c| c.kind == CallKind::FetchMatches)
        .unwrap()
        .at;
    assert_eq!(
        times,
        vec![(upcoming_first + Duration::minutes(30) - day()).num_minutes()],
        "only the upcoming job, 30 min after its last run"
    );
    assert_eq!(counts(&env).await, [1, 6, 12, 1 + 1 + 6 + 12, 1 + 6 + 12]);
}

#[tokio::test]
async fn a_long_outage_runs_each_overdue_job_once() {
    let env = env(scenarios()).await;
    let (mut s, handle) = env.scheduler(Some(BUDGET)).await;
    env.follow(&handle, "e").await;
    for minute in 0..=60 {
        env.set_minute(minute);
        s.tick().await.unwrap();
    }
    drop(s);

    // Down from 01:00 to 13:30: twelve upcoming runs and two structure
    // runs were missed. After the restart each job runs once, spread over
    // the first minute by the startup jitter.
    let before = env.source.calls().len();
    let (mut s, _handle) = env.scheduler(Some(BUDGET)).await;
    for minute in 13 * 60 + 30..=13 * 60 + 34 {
        env.set_minute(minute);
        s.tick().await.unwrap();
    }
    let kinds: Vec<CallKind> = env.source.calls()[before..]
        .iter()
        .map(|c| c.kind)
        .collect();
    let n = |k: CallKind| kinds.iter().filter(|x| **x == k).count();
    assert_eq!(n(CallKind::FetchEvent), 1, "{kinds:?}");
    assert_eq!(n(CallKind::FetchEventMatches), 1, "{kinds:?}");
    // The 14:00 matches are imminent now, the others upcoming.
    assert_eq!(n(CallKind::FetchMatches), 2, "{kinds:?}");
    assert_eq!(kinds.len(), 4, "{kinds:?}");
}
