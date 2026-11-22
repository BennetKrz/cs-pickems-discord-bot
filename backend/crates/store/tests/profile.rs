//! A member's pick history (T15) against a temp database: which segment a
//! picked match lands in, the order and paging, and the filters.

mod common;

use chrono::{DateTime, Duration, Utc};
use common::{ALICE, BOB, GUILD, at, event, match_between, seed, test_db, user};
use domain::{EventId, MatchFields, MatchFormat, MatchId, MatchStatus, PickParts};
use store::profile::{PickCounts, PickQuery, PickSegment, list_picks, pick_counts};
use store::users::GuildMember;
use store::{SqlitePool, matches, picks, profile, users};

const OTHER_GUILD: &str = "100000000000000002";
const CAROL: &str = "200000000000000003";

async fn pick(pool: &SqlitePool, guild: &str, user_id: &str, m: MatchId) {
    picks::save(pool, guild, user_id, m, &PickParts::default(), at(9, 0))
        .await
        .unwrap();
}

/// Changes a stored match.
async fn set(pool: &SqlitePool, id: MatchId, f: impl FnOnce(&mut MatchFields)) {
    let mut m = matches::get(pool, id).await.unwrap().unwrap();
    f(&mut m.fields);
    matches::update(pool, &m).await.unwrap();
}

async fn ids(
    pool: &SqlitePool,
    user_id: &str,
    segment: PickSegment,
    event_id: Option<EventId>,
) -> Vec<MatchId> {
    list_picks(
        pool,
        &PickQuery {
            guild_id: GUILD,
            user_id,
            segment,
            event_id,
            after: None,
            limit: 100,
        },
    )
    .await
    .unwrap()
    .into_iter()
    .map(|i| i.m.id)
    .collect()
}

#[tokio::test]
async fn each_picked_match_lands_in_one_segment() {
    let db = test_db().await;
    let pool = &db.pool;
    let s = seed(pool).await;
    let ev = s.event;
    let new = |pool| match_between(pool, ev, Some(s.team_a), Some(s.team_c), MatchFormat::Bo1);
    // `s.bo3` is scheduled at 18:00.
    let postponed = new(pool).await;
    set(pool, postponed, |f| f.status = MatchStatus::Postponed).await;
    let tba = new(pool).await;
    set(pool, tba, |f| f.scheduled_at = None).await;
    let live = new(pool).await;
    set(pool, live, |f| {
        f.status = MatchStatus::Live;
        f.started_at = Some(at(18, 5));
    })
    .await;
    let finished = new(pool).await;
    set(pool, finished, |f| {
        f.status = MatchStatus::Finished;
        f.ended_at = Some(at(20, 0));
    })
    .await;
    let canceled = new(pool).await;
    set(pool, canceled, |f| f.status = MatchStatus::Canceled).await;
    // Voided while still scheduled, and while live: both are settled.
    let voided = new(pool).await;
    set(pool, voided, |f| f.voided = true).await;
    let voided_live = new(pool).await;
    set(pool, voided_live, |f| {
        f.status = MatchStatus::Live;
        f.voided = true;
    })
    .await;
    // Not picked by Alice.
    let unpicked = new(pool).await;
    pick(pool, GUILD, BOB, unpicked).await;

    for m in [
        s.bo3,
        postponed,
        tba,
        live,
        finished,
        canceled,
        voided,
        voided_live,
    ] {
        pick(pool, GUILD, ALICE, m).await;
    }

    // Open: no start time first, then by start (ties by ID).
    assert_eq!(
        ids(pool, ALICE, PickSegment::Open, None).await,
        [tba, s.bo3, postponed]
    );
    assert_eq!(ids(pool, ALICE, PickSegment::Live, None).await, [live]);
    // Settled: latest first; `finished` ended at 20:00, the others are
    // dated by their 18:00 schedule, then by ID descending.
    assert_eq!(
        ids(pool, ALICE, PickSegment::Settled, None).await,
        [finished, voided_live, voided, canceled]
    );
    assert_eq!(
        pick_counts(pool, GUILD, ALICE, None).await.unwrap(),
        PickCounts {
            open: 3,
            live: 1,
            settled: 4
        }
    );
    assert_eq!(
        pick_counts(pool, GUILD, ALICE, None).await.unwrap().total(),
        8
    );
    assert_eq!(ids(pool, BOB, PickSegment::Open, None).await, [unpicked]);
}

#[tokio::test]
async fn filters_by_event_user_and_guild_also_for_unfollowed_events() {
    let db = test_db().await;
    let pool = &db.pool;
    let s = seed(pool).await;
    // Nobody follows either event: the history still lists them.
    let ev2 = event(pool, "Major").await;
    let other = match_between(pool, ev2, Some(s.team_b), Some(s.team_c), MatchFormat::Bo3).await;
    pick(pool, GUILD, ALICE, s.bo3).await;
    pick(pool, GUILD, ALICE, other).await;
    pick(pool, GUILD, BOB, other).await;
    pick(pool, OTHER_GUILD, ALICE, s.bo3).await;

    assert_eq!(
        ids(pool, ALICE, PickSegment::Open, None).await,
        [s.bo3, other]
    );
    assert_eq!(
        ids(pool, ALICE, PickSegment::Open, Some(ev2)).await,
        [other]
    );
    assert_eq!(
        pick_counts(pool, GUILD, ALICE, Some(ev2)).await.unwrap(),
        PickCounts {
            open: 1,
            live: 0,
            settled: 0
        }
    );
    // The other guild's pick is another history.
    let theirs = list_picks(
        pool,
        &PickQuery {
            guild_id: OTHER_GUILD,
            user_id: ALICE,
            segment: PickSegment::Open,
            event_id: None,
            after: None,
            limit: 10,
        },
    )
    .await
    .unwrap();
    assert_eq!(theirs.len(), 1);
    // Pick counts are the guild's, not the member's.
    assert_eq!(theirs[0].pick_count, 1);
    let ours = list_picks(
        pool,
        &PickQuery {
            guild_id: GUILD,
            user_id: ALICE,
            segment: PickSegment::Open,
            event_id: Some(ev2),
            after: None,
            limit: 10,
        },
    )
    .await
    .unwrap();
    assert_eq!(ours[0].pick_count, 2);
    assert_eq!(ours[0].event_name, "Major");
    assert_eq!(ours[0].team_a.as_ref().unwrap().name, "Bravo");
}

#[tokio::test]
async fn settled_pages_follow_the_cursor() {
    let db = test_db().await;
    let pool = &db.pool;
    let s = seed(pool).await;
    let mut all = Vec::new();
    for i in 0..5 {
        let m = match_between(
            pool,
            s.event,
            Some(s.team_a),
            Some(s.team_c),
            MatchFormat::Bo1,
        )
        .await;
        // Two matches share an end time, so the ID breaks the tie.
        let end: DateTime<Utc> = at(10, 0) + Duration::hours(i.min(3));
        set(pool, m, |f| {
            f.status = MatchStatus::Finished;
            f.ended_at = Some(end);
        })
        .await;
        pick(pool, GUILD, ALICE, m).await;
        all.push(m);
    }
    let page = |after| async move {
        list_picks(
            pool,
            &PickQuery {
                guild_id: GUILD,
                user_id: ALICE,
                segment: PickSegment::Settled,
                event_id: None,
                after,
                limit: 2,
            },
        )
        .await
        .unwrap()
    };
    let mut seen = Vec::new();
    let mut after = None;
    loop {
        let rows = page(after).await;
        if rows.is_empty() {
            break;
        }
        let last = rows.last().unwrap();
        after = Some((last.sort_at.unwrap(), last.m.id));
        seen.extend(rows.iter().map(|r| r.m.id));
    }
    // Latest first; the two at 13:00 by ID descending.
    assert_eq!(seen, [all[4], all[3], all[2], all[1], all[0]]);
}

#[tokio::test]
async fn members_are_who_signed_in_or_picked_in_the_guild() {
    let db = test_db().await;
    let pool = &db.pool;
    let s = seed(pool).await;
    user(pool, CAROL).await;
    users::upsert_member(
        pool,
        &GuildMember {
            guild_id: GUILD.into(),
            user_id: BOB.into(),
            nick: None,
            last_seen_at: at(9, 0),
        },
    )
    .await
    .unwrap();
    pick(pool, GUILD, ALICE, s.bo3).await;

    assert!(profile::is_member(pool, GUILD, ALICE).await.unwrap());
    assert!(profile::is_member(pool, GUILD, BOB).await.unwrap());
    // Known user, but never in this guild.
    assert!(!profile::is_member(pool, GUILD, CAROL).await.unwrap());
    assert!(!profile::is_member(pool, OTHER_GUILD, BOB).await.unwrap());
    assert!(!profile::is_member(pool, GUILD, "1").await.unwrap());
}
