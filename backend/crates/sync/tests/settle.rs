//! Settlement (T11) against a temp database: the fixture scenario through
//! the real reconciler and admin actions, plus hand-built matches for the
//! cases the scenario doesn't have (missing round scores, many picks).

use std::sync::Arc;
use std::time::Instant;

use chrono::{DateTime, Duration, TimeZone, Utc};
use domain::{
    Clock, DomainEvent, EntityType, EventStatus, EventTier, FakeClock, MapPick, MapStatus,
    MatchFields, MatchFormat, MatchId, MatchMap, MatchStatus, NewEvent, NewTeam, PickParts,
    ResultState, SeriesScore, TeamId,
};
use serde_json::json;
use sources::SourceAdapter;
use sources::fixture::{FIXTURE, FixtureAdapter, Scenario};
use store::SqlitePool;
use store::picks::PickResult;
use store::scoring::UserTotal;
use store::users::User;
use sync::settle::{self, RulesMode};
use sync::{EventBus, ManualService, Override, OverrideField, Precedence, Reconciler};
use tokio::sync::broadcast::Receiver;
use tokio_util::sync::CancellationToken;

const GUILD: &str = "100000000000000001";
const OTHER_GUILD: &str = "100000000000000002";
const ALICE: &str = "200000000000000001";
const BOB: &str = "200000000000000002";
const ADMIN: &str = "300000000000000001";

fn anchor() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 11, 20, 12, 0, 0).unwrap()
}

fn minute(m: i64) -> DateTime<Utc> {
    anchor() + Duration::minutes(m)
}

struct Env {
    pool: SqlitePool,
    rec: Arc<Reconciler>,
    manual: ManualService,
    bus: EventBus,
    clock: Arc<FakeClock>,
    src: FixtureAdapter,
    rx: Receiver<DomainEvent>,
    _dir: tempfile::TempDir,
}

async fn env() -> Env {
    let dir = tempfile::tempdir().unwrap();
    let url = format!("sqlite://{}", dir.path().join("test.db").display());
    let pool = store::open(&url).await.unwrap();
    let bus = EventBus::new();
    let rx = bus.subscribe();
    let rec = Arc::new(Reconciler::new(
        pool.clone(),
        Precedence::from_enabled(&[FIXTURE.to_owned()]),
        Some(FIXTURE.to_owned()),
        bus.clone(),
    ));
    let clock = Arc::new(FakeClock::new(anchor()));
    let src = FixtureAdapter::new(Scenario::default_scenario(), anchor(), clock.clone());
    for id in [ALICE, BOB] {
        user(&pool, id).await;
    }
    Env {
        pool,
        manual: ManualService::new(rec.clone()),
        rec,
        bus,
        clock,
        src,
        rx,
        _dir: dir,
    }
}

async fn user(pool: &SqlitePool, id: &str) {
    store::users::upsert_user(
        pool,
        &User {
            discord_user_id: id.to_owned(),
            username: format!("user{id}"),
            global_name: None,
            avatar_hash: None,
            updated_at: anchor(),
        },
    )
    .await
    .unwrap();
}

impl Env {
    /// Moves the clock to `m` minutes after the anchor, syncs the fixture
    /// event, and settles what the events touched, like the worker does.
    async fn step(&mut self, m: i64) {
        self.clock.set(minute(m));
        let now = self.clock.now();
        let ev = self.src.fetch_event("fx-major").await.unwrap();
        self.rec.ingest_event(FIXTURE, &ev, now).await.unwrap();
        let ms = self.src.fetch_event_matches("fx-major").await.unwrap();
        let rep = self.rec.ingest_matches(FIXTURE, &ms, now).await.unwrap();
        assert_eq!(rep.failed, 0);
        self.settle_events().await;
    }

    /// Settles the match of every event published so far; returns the
    /// events.
    async fn settle_events(&mut self) -> Vec<DomainEvent> {
        let mut events = Vec::new();
        while let Ok(e) = self.rx.try_recv() {
            settle::settle_match(
                &self.pool,
                e.match_id(),
                self.clock.now(),
                RulesMode::Pinned,
            )
            .await
            .unwrap();
            events.push(e);
        }
        events
    }

    async fn id(&self, ext: &str) -> MatchId {
        MatchId(
            store::refs::resolve(&self.pool, FIXTURE, EntityType::Match, ext)
                .await
                .unwrap()
                .unwrap(),
        )
    }

    async fn teams(&self, id: MatchId) -> (TeamId, TeamId) {
        let m = store::matches::get(&self.pool, id).await.unwrap().unwrap();
        (m.fields.team_a_id.unwrap(), m.fields.team_b_id.unwrap())
    }

    async fn pick(&self, guild: &str, user: &str, id: MatchId, parts: &PickParts) {
        store::picks::save(&self.pool, guild, user, id, parts, self.clock.now())
            .await
            .unwrap();
    }

    /// `(points, state, pending parts)` of a member's pick.
    async fn result(
        &self,
        guild: &str,
        user: &str,
        id: MatchId,
    ) -> Option<(i64, ResultState, i64)> {
        let p = store::picks::get(&self.pool, guild, user, id)
            .await
            .unwrap()
            .unwrap();
        store::picks::get_result(&self.pool, p.id)
            .await
            .unwrap()
            .map(|r| (r.points, r.state, r.pending_parts))
    }

    async fn totals(&self) -> Vec<(String, i64)> {
        store::scoring::totals_for_guild(&self.pool, GUILD)
            .await
            .unwrap()
            .into_iter()
            .map(|UserTotal { user_id, points }| (user_id, points))
            .collect()
    }
}

fn map_rounds(n: u8, winner: TeamId, rounds: (u16, u16)) -> MapPick {
    MapPick {
        map_number: n,
        winner_team_id: winner,
        rounds: Some(rounds),
        overtime: false,
    }
}

fn map_ot(n: u8, winner: TeamId) -> MapPick {
    MapPick {
        map_number: n,
        winner_team_id: winner,
        rounds: None,
        overtime: true,
    }
}

/// sw-r2-1 is a Bo3 at +180 that A wins 2:1: 13:7, 14:16 (overtime),
/// 13:10, one map every 45 minutes. Alice picks it perfectly (15 points);
/// Bob picks B everywhere (0 points).
async fn bo3_with_picks(e: &mut Env) -> MatchId {
    e.step(0).await;
    let id = e.id("sw-r2-1").await;
    let (a, b) = e.teams(id).await;
    let perfect = PickParts {
        winner_team_id: Some(a),
        series: Some(SeriesScore { a: 2, b: 1 }),
        maps: vec![
            map_rounds(1, a, (13, 7)),
            map_ot(2, b),
            map_rounds(3, a, (13, 10)),
        ],
    };
    let wrong = PickParts {
        winner_team_id: Some(b),
        series: Some(SeriesScore { a: 0, b: 2 }),
        maps: vec![map_rounds(1, b, (7, 13))],
    };
    e.pick(GUILD, ALICE, id, &perfect).await;
    e.pick(GUILD, BOB, id, &wrong).await;
    e.pick(OTHER_GUILD, ALICE, id, &wrong).await;
    id
}

// ---- acceptance criterion 3: provisional while live, final at the end ----------

#[tokio::test]
async fn map_points_are_provisional_while_live_and_final_at_match_end() {
    let mut e = env().await;
    let id = bo3_with_picks(&mut e).await;
    assert_eq!(e.result(GUILD, ALICE, id).await, None, "not started");

    e.step(180).await;
    assert_eq!(
        e.result(GUILD, ALICE, id).await,
        Some((0, ResultState::Provisional, 8)),
        "live, nothing finished"
    );

    // Map 1 (13:7) is done: map winner + exact score.
    e.step(225).await;
    assert_eq!(
        e.result(GUILD, ALICE, id).await,
        Some((3, ResultState::Provisional, 6))
    );
    assert_eq!(
        e.result(GUILD, BOB, id).await,
        Some((0, ResultState::Provisional, 2))
    );

    // Map 2 went to overtime for B: Alice's overtime pick hits.
    e.step(270).await;
    assert_eq!(
        e.result(GUILD, ALICE, id).await,
        Some((6, ResultState::Provisional, 4))
    );

    e.step(315).await;
    assert_eq!(
        e.result(GUILD, ALICE, id).await,
        Some((15, ResultState::Final, 0))
    );
    assert_eq!(
        e.result(GUILD, BOB, id).await,
        Some((0, ResultState::Final, 0))
    );
    assert_eq!(
        e.result(OTHER_GUILD, ALICE, id).await,
        Some((0, ResultState::Final, 0)),
        "each guild's pick is settled on its own"
    );
    assert_eq!(
        e.totals().await,
        vec![(ALICE.to_owned(), 15), (BOB.to_owned(), 0)]
    );
}

// ---- acceptance criterion 2: corrections and idempotence ---------------------

#[tokio::test]
async fn a_corrected_result_changes_points_and_settling_twice_changes_nothing() {
    let mut e = env().await;
    let id = bo3_with_picks(&mut e).await;
    e.step(315).await;
    assert_eq!(
        e.totals().await,
        vec![(ALICE.to_owned(), 15), (BOB.to_owned(), 0)]
    );

    // Settling again, directly or by sweep, writes nothing.
    let before = store::picks::results_for_match(&e.pool, id).await.unwrap();
    e.clock.set(minute(400));
    let rep = settle::settle_match(&e.pool, id, e.clock.now(), RulesMode::Pinned)
        .await
        .unwrap();
    assert_eq!((rep.picks, rep.written, rep.deleted), (3, 0, 0));
    let rep = settle::sweep(&e.pool, e.clock.now()).await.unwrap();
    assert_eq!(rep.written, 0);
    let after = store::picks::results_for_match(&e.pool, id).await.unwrap();
    assert_eq!(before, after, "rows untouched, settled_at included");

    // An admin corrects the series to 1:2 for B.
    e.manual
        .set(id, Override::Series { score: (1, 2) }, ADMIN, e.clock.now())
        .await
        .unwrap();
    let events = e.settle_events().await;
    assert!(events.contains(&DomainEvent::ResultCorrected { match_id: id }));
    // Alice keeps her map points (9), Bob gets the winner (3).
    assert_eq!(
        e.totals().await,
        vec![(ALICE.to_owned(), 9), (BOB.to_owned(), 3)]
    );

    // Taking the correction back restores the original points.
    e.manual
        .clear(id, OverrideField::Series, ADMIN, e.clock.now())
        .await
        .unwrap();
    e.settle_events().await;
    assert_eq!(
        e.totals().await,
        vec![(ALICE.to_owned(), 15), (BOB.to_owned(), 0)]
    );
}

// ---- void cases ---------------------------------------------------------------

#[tokio::test]
async fn void_unvoid_and_cancel() {
    let mut e = env().await;
    let id = bo3_with_picks(&mut e).await;
    // sw-r1-4 is canceled at +60, like a walkover.
    let canceled = e.id("sw-r1-4").await;
    let (a, _) = e.teams(canceled).await;
    let winner = PickParts {
        winner_team_id: Some(a),
        ..PickParts::default()
    };
    e.pick(GUILD, ALICE, canceled, &winner).await;
    e.step(315).await;
    assert_eq!(
        e.result(GUILD, ALICE, canceled).await,
        Some((0, ResultState::Void, 0))
    );

    e.manual.void(id, ADMIN, e.clock.now()).await.unwrap();
    e.settle_events().await;
    assert_eq!(
        e.result(GUILD, ALICE, id).await,
        Some((0, ResultState::Void, 0))
    );
    assert_eq!(
        e.totals().await,
        vec![(ALICE.to_owned(), 0), (BOB.to_owned(), 0)]
    );

    e.manual.unvoid(id, ADMIN, e.clock.now()).await.unwrap();
    e.settle_events().await;
    assert_eq!(
        e.result(GUILD, ALICE, id).await,
        Some((15, ResultState::Final, 0))
    );
}

#[tokio::test]
async fn a_match_open_again_loses_its_results() {
    let mut e = env().await;
    e.step(0).await;
    // sw-r1-3: a Bo1 at +90.
    let id = e.id("sw-r1-3").await;
    let (a, _) = e.teams(id).await;
    e.pick(
        GUILD,
        ALICE,
        id,
        &PickParts {
            winner_team_id: Some(a),
            ..PickParts::default()
        },
    )
    .await;

    // Canceled by an admin: void results and a rules pin.
    let cancel = Override::Status {
        status: MatchStatus::Canceled,
        started_at: None,
        ended_at: None,
    };
    e.manual
        .set(id, cancel, ADMIN, e.clock.now())
        .await
        .unwrap();
    e.settle_events().await;
    assert_eq!(
        e.result(GUILD, ALICE, id).await,
        Some((0, ResultState::Void, 0))
    );
    assert!(
        store::scoring::get_pin(&e.pool, GUILD, id)
            .await
            .unwrap()
            .is_some()
    );

    // The lock time passes; then the admin moves the match, takes the cancel
    // back and reopens the picks.
    e.clock.set(minute(95));
    store::matches::lock_due(&e.pool, e.clock.now())
        .await
        .unwrap();
    let later = Override::Schedule {
        scheduled_at: minute(500),
    };
    e.manual.set(id, later, ADMIN, e.clock.now()).await.unwrap();
    e.manual
        .clear(id, OverrideField::Status, ADMIN, e.clock.now())
        .await
        .unwrap();
    e.manual.reopen(id, ADMIN, e.clock.now()).await.unwrap();
    let events = e.settle_events().await;
    assert!(events.contains(&DomainEvent::PicksReopened { match_id: id }));

    assert_eq!(e.result(GUILD, ALICE, id).await, None);
    assert!(
        store::scoring::get_pin(&e.pool, GUILD, id)
            .await
            .unwrap()
            .is_none(),
        "the next lock picks the rules anew"
    );
}

// ---- rules snapshot -------------------------------------------------------------

#[tokio::test]
async fn rules_are_pinned_at_lock_and_rescore_applies_the_current_ones() {
    let mut e = env().await;
    let id = bo3_with_picks(&mut e).await;
    // Before the lock: version 2 applies to this match.
    let v2 = store::scoring::set_rules(&e.pool, GUILD, &json!({ "winner": 10 }), e.clock.now())
        .await
        .unwrap();
    assert_eq!(v2, 2);
    e.step(180).await;

    // After the lock: version 3 does not reshuffle this match.
    e.clock.set(minute(200));
    store::scoring::set_rules(&e.pool, GUILD, &json!({ "winner": 100 }), e.clock.now())
        .await
        .unwrap();
    e.step(315).await;
    let results = store::picks::results_for_match(&e.pool, id).await.unwrap();
    let alice_pick = store::picks::get(&e.pool, GUILD, ALICE, id)
        .await
        .unwrap()
        .unwrap();
    let r: &PickResult = results.iter().find(|r| r.pick_id == alice_pick.id).unwrap();
    assert_eq!((r.points, r.rules_version), (22, 2), "10 + 3 + 9");
    // The other guild never changed its rules.
    assert_eq!(
        e.result(OTHER_GUILD, ALICE, id).await,
        Some((0, ResultState::Final, 0))
    );

    let rep = settle::rescore_all(&e.pool, e.clock.now()).await.unwrap();
    assert!(rep.written >= 1);
    let r = store::picks::get_result(&e.pool, alice_pick.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!((r.points, r.rules_version), (112, 3));

    // Later settlements keep the version rescore chose.
    e.clock.set(minute(400));
    let rep = settle::settle_match(&e.pool, id, e.clock.now(), RulesMode::Pinned)
        .await
        .unwrap();
    assert_eq!(rep.written, 0);
}

// ---- hand-built matches -----------------------------------------------------------

/// A finished Bo5 between two new teams, ended at `ended`, that A won 3:1.
/// Maps have winners; `rounds` decides whether they have round scores.
async fn finished_bo5(
    pool: &SqlitePool,
    ended: DateTime<Utc>,
    rounds: bool,
) -> (MatchId, TeamId, TeamId) {
    let team = async |name: &str| {
        store::teams::insert(
            pool,
            &NewTeam {
                name: name.to_owned(),
                acronym: None,
                logo_url: None,
                country: None,
            },
        )
        .await
        .unwrap()
    };
    let (a, b) = (team("Alpha").await, team("Bravo").await);
    let event = store::events::insert(
        pool,
        &NewEvent {
            name: "Built Cup".to_owned(),
            tier: EventTier::A,
            begin_at: None,
            end_at: None,
            status: EventStatus::Running,
        },
    )
    .await
    .unwrap();
    let start = ended - Duration::hours(4);
    let mut f = MatchFields::scheduled(event, Some(a), Some(b), MatchFormat::Bo5, Some(start));
    f.status = MatchStatus::Finished;
    f.started_at = Some(start);
    f.ended_at = Some(ended);
    f.locked = true;
    f.maps_won_a = 3;
    f.maps_won_b = 1;
    f.winner_team_id = Some(a);
    let id = store::matches::insert(pool, &f).await.unwrap();
    let results = [(a, (13, 5)), (b, (9, 13)), (a, (16, 14)), (a, (13, 11))];
    for (i, (w, r)) in results.into_iter().enumerate() {
        store::matches::upsert_map(
            pool,
            &MatchMap {
                match_id: id,
                number: u8::try_from(i + 1).unwrap(),
                map_name: None,
                status: MapStatus::Finished,
                rounds_a: rounds.then_some(r.0),
                rounds_b: rounds.then_some(r.1),
                winner_team_id: Some(w),
            },
        )
        .await
        .unwrap();
    }
    store::matches::upsert_map(
        pool,
        &MatchMap {
            match_id: id,
            number: 5,
            map_name: None,
            status: MapStatus::NotPlayed,
            rounds_a: None,
            rounds_b: None,
            winner_team_id: None,
        },
    )
    .await
    .unwrap();
    (id, a, b)
}

/// A full Bo5 pick: A 3:1, every map with a score.
fn full_bo5(a: TeamId, b: TeamId) -> PickParts {
    PickParts {
        winner_team_id: Some(a),
        series: Some(SeriesScore { a: 3, b: 1 }),
        maps: vec![
            map_rounds(1, a, (13, 5)),
            map_rounds(2, b, (9, 13)),
            map_ot(3, a),
            map_rounds(4, a, (13, 11)),
            map_rounds(5, b, (3, 13)),
        ],
    }
}

#[tokio::test]
async fn missing_round_scores_stay_pending_then_become_void_after_48_hours() {
    let e = env().await;
    let ended = minute(0);
    let (id, a, b) = finished_bo5(&e.pool, ended, false).await;
    e.pick(GUILD, ALICE, id, &full_bo5(a, b)).await;

    // Winner 3 + series 3 + 4 map winners; 4 score parts wait, map 5 void.
    settle::sweep(&e.pool, ended + Duration::hours(1))
        .await
        .unwrap();
    assert_eq!(
        e.result(GUILD, ALICE, id).await,
        Some((10, ResultState::Final, 4))
    );
    let rep = settle::sweep(&e.pool, ended + Duration::hours(47))
        .await
        .unwrap();
    assert_eq!((rep.matches, rep.written), (1, 0), "still waiting");

    let rep = settle::sweep(&e.pool, ended + Duration::hours(48))
        .await
        .unwrap();
    assert_eq!(rep.written, 1);
    assert_eq!(
        e.result(GUILD, ALICE, id).await,
        Some((10, ResultState::Final, 0))
    );
    let r = store::picks::results_for_match(&e.pool, id).await.unwrap();
    assert_eq!(r[0].breakdown["maps"][0]["score"]["outcome"], "void");
    assert_eq!(r[0].breakdown["maps"][4]["winner"]["outcome"], "void");
    assert!(
        store::scoring::matches_to_settle(&e.pool)
            .await
            .unwrap()
            .is_empty()
    );
}

// ---- acceptance criterion 4: speed ------------------------------------------------

#[tokio::test]
async fn settling_a_match_with_50_picks_takes_under_a_second() {
    let e = env().await;
    let (id, a, b) = finished_bo5(&e.pool, minute(0), true).await;
    for i in 0..50 {
        let uid = format!("2100000000000000{i:02}");
        user(&e.pool, &uid).await;
        e.pick(GUILD, &uid, id, &full_bo5(a, b)).await;
    }
    let started = Instant::now();
    let rep = settle::settle_match(&e.pool, id, minute(60), RulesMode::Pinned)
        .await
        .unwrap();
    let took = started.elapsed();
    eprintln!("settled 50 picks in {took:?}");
    assert_eq!((rep.picks, rep.written), (50, 50));
    assert!(took < std::time::Duration::from_secs(1), "took {took:?}");
    // 3 + 3 + (1+2) + (1+2) + (1+2) + (1+2) + void = 18.
    assert_eq!(e.totals().await[0].1, 18);
}

// ---- the worker -----------------------------------------------------------------------

#[tokio::test]
async fn the_worker_catches_up_at_start_and_settles_on_events() {
    let e = env().await;
    let (id, a, b) = finished_bo5(&e.pool, minute(0), true).await;
    e.pick(GUILD, ALICE, id, &full_bo5(a, b)).await;
    e.clock.set(minute(60));

    let cancel = CancellationToken::new();
    let worker = tokio::spawn(settle::run_settler(
        e.pool.clone(),
        e.clock.clone(),
        e.bus.subscribe(),
        std::time::Duration::from_secs(3600),
        cancel.clone(),
    ));

    let wait_for = async |want: (i64, ResultState, i64)| {
        for _ in 0..200 {
            if e.result(GUILD, ALICE, id).await == Some(want) {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        panic!(
            "expected {want:?}, got {:?}",
            e.result(GUILD, ALICE, id).await
        );
    };
    // The start-up sweep settles the finished match nobody announced.
    wait_for((18, ResultState::Final, 0)).await;

    // An admin void reaches the worker as an event.
    e.manual.void(id, ADMIN, e.clock.now()).await.unwrap();
    wait_for((0, ResultState::Void, 0)).await;

    cancel.cancel();
    worker.await.unwrap();
}
