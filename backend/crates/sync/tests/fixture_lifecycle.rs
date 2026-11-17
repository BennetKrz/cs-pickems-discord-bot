//! The fixture scenario through the real reconciler (T05 acceptance
//! criterion 3): it loads, and advancing a fake clock drives full match
//! lifecycles with the normal domain events.

use std::sync::Arc;

use chrono::{DateTime, Duration, TimeZone, Utc};
use domain::{
    Clock, DomainEvent, EntityType, FakeClock, MapStatus, MatchFormat, MatchId, MatchStatus,
    StageKind,
};
use sources::SourceAdapter;
use sources::fixture::{FIXTURE, FixtureAdapter, Scenario};
use store::SqlitePool;
use sync::{EventBus, Precedence, Reconciler};
use tokio::sync::broadcast::Receiver;

struct Env {
    pool: SqlitePool,
    rec: Reconciler,
    clock: Arc<FakeClock>,
    src: FixtureAdapter,
    rx: Receiver<DomainEvent>,
    _dir: tempfile::TempDir,
}

fn anchor() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 11, 20, 12, 0, 0).unwrap()
}

async fn env() -> Env {
    let dir = tempfile::tempdir().unwrap();
    let url = format!("sqlite://{}", dir.path().join("test.db").display());
    let pool = store::open(&url).await.unwrap();
    let bus = EventBus::new();
    let rx = bus.subscribe();
    let rec = Reconciler::new(
        pool.clone(),
        Precedence::from_enabled(&[FIXTURE.to_owned()]),
        Some(FIXTURE.to_owned()),
        bus,
    );
    let clock = Arc::new(FakeClock::new(anchor()));
    let src = FixtureAdapter::new(Scenario::default_scenario(), anchor(), clock.clone());
    Env {
        pool,
        rec,
        clock,
        src,
        rx,
        _dir: dir,
    }
}

impl Env {
    /// Moves the clock to `minutes` after the anchor, syncs the whole event
    /// like the scheduler would, and returns the published events.
    async fn step(&mut self, minutes: i64) -> Vec<DomainEvent> {
        self.clock.set(anchor() + Duration::minutes(minutes));
        let now = self.clock.now();
        let ev = self.src.fetch_event("fx-major").await.unwrap();
        let rep = self.rec.ingest_event(FIXTURE, &ev, now).await.unwrap();
        assert_eq!(rep.failed, 0);
        let ms = self.src.fetch_event_matches("fx-major").await.unwrap();
        let rep = self.rec.ingest_matches(FIXTURE, &ms, now).await.unwrap();
        assert_eq!(rep.failed, 0);
        assert_eq!(rep.queued, 0);
        let mut v = Vec::new();
        while let Ok(e) = self.rx.try_recv() {
            v.push(e);
        }
        v
    }

    async fn id(&self, ext: &str) -> MatchId {
        MatchId(
            store::refs::resolve(&self.pool, FIXTURE, EntityType::Match, ext)
                .await
                .unwrap()
                .unwrap(),
        )
    }

    async fn get(&self, ext: &str) -> domain::Match {
        store::matches::get(&self.pool, self.id(ext).await)
            .await
            .unwrap()
            .unwrap()
    }
}

/// Events of one match.
fn of(events: &[DomainEvent], id: MatchId) -> Vec<DomainEvent> {
    events
        .iter()
        .filter(|e| e.match_id() == id)
        .cloned()
        .collect()
}

#[tokio::test]
async fn scenario_loads_with_all_required_cases() {
    let mut e = env().await;
    let events = e.step(0).await;
    assert_eq!(events.len(), 12, "one MatchCreated per match");
    assert!(
        events
            .iter()
            .all(|e| matches!(e, DomainEvent::MatchCreated { .. }))
    );

    let event_id = store::refs::resolve(&e.pool, FIXTURE, EntityType::Event, "fx-major")
        .await
        .unwrap()
        .unwrap();
    let stages = store::events::list_stages(&e.pool, domain::EventId(event_id))
        .await
        .unwrap();
    let kinds: Vec<_> = stages.iter().map(|s| s.kind).collect();
    assert_eq!(kinds, [StageKind::Swiss, StageKind::DoubleElim]);

    let formats = [
        ("sw-r1-1", MatchFormat::Bo1),
        ("sw-r2-1", MatchFormat::Bo3),
        ("gf", MatchFormat::Bo5),
    ];
    for (ext, f) in formats {
        assert_eq!(e.get(ext).await.fields.format, f, "{ext}");
    }

    // TBD slots with resolved bracket feeds.
    let uf = e.get("ub-f").await;
    assert_eq!((uf.fields.team_a_id, uf.fields.team_b_id), (None, None));
    let feed = uf.fields.bracket.feeds_a_from.unwrap();
    assert_eq!(feed.match_id, e.id("ub-sf1").await);

    // Re-syncing without moving the clock changes nothing.
    assert!(e.step(0).await.is_empty());
}

#[tokio::test]
async fn advancing_the_clock_drives_a_full_lifecycle() {
    let mut e = env().await;
    e.step(0).await;
    let bo1 = e.id("sw-r1-1").await;
    let canceled = e.id("sw-r1-4").await;

    // +30: the Bo1 starts; its map is now known and live.
    let ev = e.step(30).await;
    assert_eq!(
        of(&ev, bo1),
        vec![
            DomainEvent::MatchStarted { match_id: bo1 },
            DomainEvent::MapUpdated {
                match_id: bo1,
                number: 1
            },
        ]
    );

    // +60: the other match is canceled.
    let ev = e.step(60).await;
    assert_eq!(
        of(&ev, canceled),
        vec![DomainEvent::MatchCanceled { match_id: canceled }]
    );
    assert!(of(&ev, bo1).is_empty(), "nothing new for the Bo1");

    // +75: the Bo1's only map is done, and so is the match.
    let ev = e.step(75).await;
    assert_eq!(
        of(&ev, bo1),
        vec![
            DomainEvent::MatchFinished { match_id: bo1 },
            DomainEvent::MapUpdated {
                match_id: bo1,
                number: 1
            },
        ]
    );
    let m = e.get("sw-r1-1").await;
    assert_eq!(m.fields.status, MatchStatus::Finished);
    assert_eq!((m.fields.maps_won_a, m.fields.maps_won_b), (1, 0));
    assert_eq!(m.fields.winner_team_id, m.fields.team_a_id);
    let maps = store::matches::list_maps(&e.pool, bo1).await.unwrap();
    assert_eq!((maps[0].rounds_a, maps[0].rounds_b), (Some(13), Some(9)));
    assert_eq!(maps[0].map_name.as_deref(), Some("Mirage"));

    // The Bo3 with an overtime map: maps finish one by one.
    let bo3 = e.id("sw-r2-1").await;
    let ev = e.step(180).await;
    assert!(of(&ev, bo3).contains(&DomainEvent::MatchStarted { match_id: bo3 }));
    e.step(225).await;
    let ev = e.step(270).await;
    assert_eq!(
        of(&ev, bo3),
        vec![
            DomainEvent::MapUpdated {
                match_id: bo3,
                number: 2
            },
            DomainEvent::MapUpdated {
                match_id: bo3,
                number: 3
            },
        ]
    );
    let maps = store::matches::list_maps(&e.pool, bo3).await.unwrap();
    assert_eq!(
        (maps[1].status, maps[1].rounds_a, maps[1].rounds_b),
        (MapStatus::Finished, Some(14), Some(16)),
        "overtime"
    );
    let ev = e.step(315).await;
    assert!(of(&ev, bo3).contains(&DomainEvent::MatchFinished { match_id: bo3 }));
    assert_eq!(e.get("sw-r2-1").await.fields.maps_won_a, 2);

    // A TBD slot fills once its feeder finishes.
    e.step(570).await;
    let uf = e.get("ub-f").await;
    assert_eq!(
        uf.fields.team_a_id,
        e.get("ub-sf1").await.fields.winner_team_id
    );
    assert_eq!(uf.fields.team_b_id, None);

    // At the end, the Bo5 final is decided and its last map not played.
    e.step(10_000).await;
    let gf = e.get("gf").await;
    assert_eq!(gf.fields.status, MatchStatus::Finished);
    assert_eq!((gf.fields.maps_won_a, gf.fields.maps_won_b), (3, 1));
    let maps = store::matches::list_maps(&e.pool, gf.id).await.unwrap();
    assert_eq!(maps.len(), 5);
    assert_eq!(maps[4].status, MapStatus::NotPlayed);
    assert!(e.step(10_000).await.is_empty(), "stable at the end");
}
