//! Following and unfollowing events (T09) with the scheduler on a fake
//! clock: a search result becomes a followed event whose matches the next
//! tick brings; unfollowing stops the sync and leaves picks and points.

mod common;

use std::sync::Arc;

use chrono::Utc;
use common::{ADMIN, BUDGET, Env, GUILD, SIM, env, scenario};
use domain::{Clock, EventId, PickParts};
use sources::fake::FakeAdapter;
use sources::{CallKind, SrcEvent};
use store::matches::{ListQuery, Segment};
use store::users::User;
use sync::follow::{self, FollowError};
use sync::{SearchHit, SourceEntry, SyncHandle};

const ALICE: &str = "200000000000000099";

fn sim_entry(env: &Env, search_only: bool) -> SourceEntry {
    SourceEntry {
        adapter: env.source.clone(),
        budget: Some(BUDGET),
        search_only,
    }
}

fn count_calls(env: &Env, kind: CallKind) -> usize {
    env.source.calls().iter().filter(|c| c.kind == kind).count()
}

async fn hit(handle: &SyncHandle, query: &str) -> SearchHit {
    let out = handle.search(query).await;
    assert!(out.skipped.is_empty(), "{out:?}");
    assert_eq!(out.hits.len(), 1, "{out:?}");
    out.hits.into_iter().next().unwrap()
}

async fn upcoming_ids(env: &Env) -> Vec<i64> {
    store::matches::list_view(
        &env.pool,
        &ListQuery {
            guild_id: GUILD,
            segment: Segment::Upcoming,
            bound: None,
            event_id: None,
            after: None,
            limit: 50,
        },
    )
    .await
    .unwrap()
    .into_iter()
    .map(|i| i.m.id.0)
    .collect()
}

#[tokio::test]
async fn following_a_search_result_syncs_its_matches_at_once() {
    let env = env(vec![scenario("a", &[600, 900])]).await;
    let (mut scheduler, handle) = env.scheduler(Some(BUDGET)).await;

    let h = hit(&handle, "event a").await;
    assert_eq!(h.source, SIM);
    let now = env.clock.now();
    let out = follow::follow_hit(&env.reconciler, Some(&handle), GUILD, ADMIN, &h, now)
        .await
        .unwrap();
    assert!(out.newly_followed);
    assert_eq!(out.event.name, "Event a");
    let id = out.event.id;
    assert!(
        store::guilds::is_followed(&env.pool, GUILD, id)
            .await
            .unwrap()
    );
    // The search result was enough to create the event: no extra request.
    assert_eq!(env.source.calls().len(), 1, "only the search");
    assert!(
        upcoming_ids(&env).await.is_empty(),
        "no matches before the sync"
    );

    // The next tick runs the structure sync right away.
    let rep = scheduler.tick().await.unwrap();
    assert!(rep.ran >= 1, "{rep:?}");
    assert_eq!(count_calls(&env, CallKind::FetchEvent), 1);
    assert_eq!(count_calls(&env, CallKind::FetchEventMatches), 1);
    assert_eq!(upcoming_ids(&env).await.len(), 2);

    // Following again changes nothing and asks for no sync.
    let again = follow::follow_hit(&env.reconciler, Some(&handle), GUILD, ADMIN, &h, now)
        .await
        .unwrap();
    assert!(!again.newly_followed);
    assert_eq!(again.event.id, id);
    let audit = store::audit::list_for_entity(&env.pool, "event", &id.0.to_string())
        .await
        .unwrap();
    let actions: Vec<&str> = audit.iter().map(|a| a.record.action.as_str()).collect();
    assert_eq!(actions, ["follow_event"]);
    assert_eq!(audit[0].record.actor, ADMIN);
    assert_eq!(audit[0].record.after.as_ref().unwrap()["guild_id"], GUILD);
}

#[tokio::test]
async fn a_result_of_a_non_primary_source_needs_a_link_first() {
    let env = env(vec![scenario("a", &[600])]).await;
    let beta = Arc::new(FakeAdapter::new("beta"));
    beta.put_event(SrcEvent::new("x1", "Beta Cup"));
    let (_s, handle) = env
        .scheduler_over(vec![
            sim_entry(&env, false),
            SourceEntry {
                adapter: beta.clone(),
                budget: None,
                search_only: false,
            },
        ])
        .await;

    let h = hit(&handle, "beta cup").await;
    assert_eq!(h.source, "beta");
    let err = follow::follow_hit(&env.reconciler, Some(&handle), GUILD, ADMIN, &h, Utc::now())
        .await
        .unwrap_err();
    assert!(
        matches!(&err, FollowError::NeedsLink { source_id, name } if source_id == "beta" && name == "Beta Cup"),
        "{err:?}"
    );
    assert!(
        store::guilds::list_followed(&env.pool, GUILD)
            .await
            .unwrap()
            .is_empty()
    );
    let queue = store::unlinked::list(&env.pool, Some("beta"))
        .await
        .unwrap();
    assert_eq!(queue.len(), 1, "the event waits for a manual link");
}

#[tokio::test]
async fn following_an_unknown_event_fails() {
    let env = env(vec![scenario("a", &[600])]).await;
    let err = follow::follow(&env.pool, None, GUILD, ADMIN, EventId(42), Utc::now())
        .await
        .unwrap_err();
    assert!(
        matches!(err, FollowError::UnknownEvent(EventId(42))),
        "{err:?}"
    );
    let err = follow::unfollow(&env.pool, GUILD, ADMIN, EventId(42), Utc::now())
        .await
        .unwrap_err();
    assert!(
        matches!(err, FollowError::UnknownEvent(EventId(42))),
        "{err:?}"
    );
}

/// AC 3: one request per source, and the repeat comes from the cache.
#[tokio::test]
async fn a_search_costs_one_request_per_source_and_repeats_from_cache() {
    let env = env(vec![scenario("a", &[600])]).await;
    let beta = Arc::new(FakeAdapter::new("beta"));
    beta.put_event(SrcEvent::new("x1", "Event a qualifier"));
    let (_s, handle) = env
        .scheduler_over(vec![
            sim_entry(&env, false),
            SourceEntry {
                adapter: beta.clone(),
                budget: Some(BUDGET),
                search_only: false,
            },
        ])
        .await;

    let first = handle.search("Event A").await;
    let sources: Vec<&str> = first.hits.iter().map(|h| h.source).collect();
    assert_eq!(sources, [SIM, "beta"]);
    assert_eq!(count_calls(&env, CallKind::SearchEvents), 1);
    assert_eq!(beta.calls(), 1);

    for q in ["event a", "  EVENT   a  "] {
        assert_eq!(handle.search(q).await, first);
    }
    assert_eq!(count_calls(&env, CallKind::SearchEvents), 1);
    assert_eq!(beta.calls(), 1);
}

/// The dev fixture source is search-only: it answers searches, but no job
/// polls it, also for a followed event it is linked to.
#[tokio::test]
async fn a_search_only_source_is_searched_but_never_polled() {
    let env = env(vec![scenario("a", &[30, 60])]).await;
    let (mut scheduler, handle) = env.scheduler_over(vec![sim_entry(&env, true)]).await;

    let h = hit(&handle, "event a").await;
    follow::follow_hit(
        &env.reconciler,
        Some(&handle),
        GUILD,
        ADMIN,
        &h,
        env.clock.now(),
    )
    .await
    .unwrap();
    env.catch_up(&["a"]).await;
    let before = env.source.calls().len();
    for minute in [1, 10, 40, 70, 300, 600] {
        env.set_minute(minute);
        let rep = scheduler.tick().await.unwrap();
        assert_eq!(rep.ran, 0, "minute {minute}: {rep:?}");
    }
    assert_eq!(env.source.calls().len(), before, "no job called the source");
}

/// AC 2: unfollowing hides the event and stops its sync, but picks, points
/// and the leaderboard stay as they were.
#[tokio::test]
async fn unfollowing_keeps_picks_and_the_leaderboard() {
    let env = env(vec![scenario("a", &[600, 900])]).await;
    let (mut scheduler, handle) = env.scheduler(Some(BUDGET)).await;
    let h = hit(&handle, "event a").await;
    let id = follow::follow_hit(
        &env.reconciler,
        Some(&handle),
        GUILD,
        ADMIN,
        &h,
        env.clock.now(),
    )
    .await
    .unwrap()
    .event
    .id;
    scheduler.tick().await.unwrap();

    store::users::upsert_user(
        &env.pool,
        &User {
            discord_user_id: ALICE.into(),
            username: "alice".into(),
            global_name: None,
            avatar_hash: None,
            updated_at: env.clock.now(),
        },
    )
    .await
    .unwrap();
    let matches = store::matches::list_for_event(&env.pool, id).await.unwrap();
    assert_eq!(matches.len(), 2);
    for m in &matches {
        let parts = PickParts {
            winner_team_id: m.fields.team_a_id,
            series: None,
            maps: Vec::new(),
        };
        store::picks::save(&env.pool, GUILD, ALICE, m.id, &parts, env.clock.now())
            .await
            .unwrap();
    }
    assert_eq!(
        store::picks::count_in_event(&env.pool, GUILD, id)
            .await
            .unwrap(),
        2
    );

    // The first match finishes and is settled; the second is upcoming.
    let now = env.set_minute(600 + 200);
    env.catch_up(&["a"]).await;
    sync::settle::sweep(&env.pool, now).await.unwrap();
    let totals = store::scoring::totals_for_guild(&env.pool, GUILD)
        .await
        .unwrap();
    assert_eq!(totals.len(), 1);
    assert!(totals[0].points > 0, "{totals:?}");
    assert_eq!(upcoming_ids(&env).await, vec![matches[1].id.0]);

    let out = follow::unfollow(&env.pool, GUILD, ADMIN, id, now)
        .await
        .unwrap();
    assert!(out.was_followed);
    assert!(
        !follow::unfollow(&env.pool, GUILD, ADMIN, id, now)
            .await
            .unwrap()
            .was_followed
    );

    assert_eq!(
        store::scoring::totals_for_guild(&env.pool, GUILD)
            .await
            .unwrap(),
        totals,
        "the leaderboard is unchanged"
    );
    assert_eq!(
        store::picks::count_in_event(&env.pool, GUILD, id)
            .await
            .unwrap(),
        2,
        "picks stay"
    );
    assert!(upcoming_ids(&env).await.is_empty(), "hidden from Upcoming");
    // A sweep after unfollowing changes no points either.
    sync::settle::sweep(&env.pool, now).await.unwrap();
    assert_eq!(
        store::scoring::totals_for_guild(&env.pool, GUILD)
            .await
            .unwrap(),
        totals
    );

    // No more polling for the event.
    let before = env.source.calls().len();
    for minute in [810, 880, 900, 960, 1100] {
        env.set_minute(minute);
        scheduler.tick().await.unwrap();
    }
    assert_eq!(env.source.calls().len(), before, "the sync stopped");

    let audit = store::audit::list_for_entity(&env.pool, "event", &id.0.to_string())
        .await
        .unwrap();
    let actions: Vec<&str> = audit.iter().map(|a| a.record.action.as_str()).collect();
    assert_eq!(actions, ["follow_event", "unfollow_event"]);
}
