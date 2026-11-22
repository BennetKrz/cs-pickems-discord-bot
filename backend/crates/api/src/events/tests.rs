//! The event view over HTTP (T13), on the fake Discord and a fake clock at
//! 09:00 on 2026-11-20.

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use chrono::{DateTime, Duration, TimeZone, Utc};
use domain::{
    BracketPos, EventId, EventStatus, EventTier, MatchFields, MatchFormat, MatchStatus, NewEvent,
    NewStage, NewTeam, StageKind, TeamId,
};
use serde_json::Value;
use store::guilds::FollowedEvent;
use tower::ServiceExt;

use crate::dev::{FixtureDriver, Scenario};
use crate::test_util::{ALICE, AuthApp, GUILD, OTHER_GUILD, OWNER, auth_app, send, t0};

fn at(h: u32, m: u32) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 11, 20, h, m, 0).unwrap()
}

async fn event(
    app: &AuthApp,
    name: &str,
    status: EventStatus,
    begin: Option<DateTime<Utc>>,
    end: Option<DateTime<Utc>>,
) -> EventId {
    store::events::insert(
        &app.pool,
        &NewEvent {
            name: name.into(),
            tier: EventTier::A,
            begin_at: begin,
            end_at: end,
            status,
        },
    )
    .await
    .unwrap()
}

async fn follow(app: &AuthApp, guild: &str, ev: EventId) {
    store::guilds::follow_event(
        &app.pool,
        &FollowedEvent {
            guild_id: guild.into(),
            event_id: ev,
            followed_by: OWNER.into(),
            followed_at: at(8, 0),
        },
    )
    .await
    .unwrap();
}

async fn team(app: &AuthApp, name: &str) -> TeamId {
    store::teams::insert(
        &app.pool,
        &NewTeam {
            name: name.into(),
            acronym: Some(name[..3].to_uppercase()),
            logo_url: None,
            country: None,
        },
    )
    .await
    .unwrap()
}

async fn get(app: &AuthApp, token: &str, uri: &str) -> Value {
    let (status, body) = app.get(uri, token).await;
    assert_eq!(status, StatusCode::OK, "{uri}: {body}");
    body
}

fn names(body: &Value) -> Vec<&str> {
    body["events"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["name"].as_str().unwrap())
        .collect()
}

#[tokio::test]
async fn followed_events_running_first_then_upcoming_then_finished() {
    use EventStatus::*;
    let app = auth_app(None).await;
    let d = |day: i64| Some(at(12, 0) + Duration::days(day));
    let old = event(&app, "Old Finals", Finished, d(-40), d(-30)).await;
    let recent = event(&app, "Recent Finals", Finished, d(-10), d(-3)).await;
    let later = event(&app, "Later Cup", Upcoming, d(20), None).await;
    let soon = event(&app, "Soon Cup", Upcoming, d(2), None).await;
    let tba = event(&app, "TBA Cup", Upcoming, None, None).await;
    let running = event(&app, "Running Major", Running, d(-1), d(5)).await;
    let canceled = event(&app, "Canceled Open", Canceled, d(-5), None).await;
    let theirs = event(&app, "Their Cup", Running, d(0), None).await;
    for ev in [old, recent, later, soon, tba, running, canceled] {
        follow(&app, GUILD, ev).await;
    }
    follow(&app, OTHER_GUILD, theirs).await;
    let mut live = MatchFields::scheduled(running, None, None, MatchFormat::Bo3, Some(at(8, 0)));
    live.status = MatchStatus::Live;
    store::matches::insert(&app.pool, &live).await.unwrap();
    let token = app.sign_in(ALICE, GUILD).await;

    let body = get(&app, &token, "/api/events").await;
    assert_eq!(
        names(&body),
        [
            "Running Major",
            "Soon Cup",
            "Later Cup",
            "TBA Cup",
            "Recent Finals",
            "Canceled Open",
            "Old Finals"
        ]
    );
    let first = &body["events"][0];
    assert_eq!(first["id"], running.0);
    assert_eq!(first["status"], "running");
    assert_eq!(first["tier"], "a");
    assert_eq!(
        (first["matches"].as_i64(), first["live"].as_i64()),
        (Some(1), Some(1))
    );
}

#[tokio::test]
async fn detail_of_an_unfollowed_event_is_readable_but_not_pickable() {
    let app = auth_app(None).await;
    let ev = event(&app, "Groups Cup", EventStatus::Running, None, None).await;
    let (a, b, c) = (
        team(&app, "Alpha").await,
        team(&app, "Bravo").await,
        team(&app, "Charlie").await,
    );
    let groups = store::events::insert_stage(
        &app.pool,
        &NewStage {
            event_id: ev,
            name: "Group A".into(),
            kind: StageKind::Groups,
            position: 0,
        },
    )
    .await
    .unwrap();
    let other = store::events::insert_stage(
        &app.pool,
        &NewStage {
            event_id: ev,
            name: "Showmatch".into(),
            kind: StageKind::Other,
            position: 1,
        },
    )
    .await
    .unwrap();
    let add = |stage, a: TeamId, b: TeamId, h: u32, done: bool| {
        let pool = app.pool.clone();
        async move {
            let mut f =
                MatchFields::scheduled(ev, Some(a), Some(b), MatchFormat::Bo1, Some(at(h, 0)));
            f.stage_id = stage;
            if done {
                f.status = MatchStatus::Finished;
                f.maps_won_a = 1;
                f.winner_team_id = Some(a);
            }
            store::matches::insert(&pool, &f).await.unwrap().0
        }
    };
    add(Some(groups), b, c, 7, true).await;
    let open = add(Some(groups), a, b, 18, false).await;
    add(Some(other), a, c, 20, false).await;
    add(None, c, a, 21, false).await;
    let token = app.sign_in(ALICE, GUILD).await;

    let body = get(&app, &token, &format!("/api/events/{}", ev.0)).await;
    assert_eq!(body["event"]["name"], "Groups Cup");
    assert_eq!(body["followed"], false);
    let stages = body["stages"].as_array().unwrap();
    let views: Vec<(&str, &str)> = stages
        .iter()
        .map(|s| (s["name"].as_str().unwrap(), s["view"].as_str().unwrap()))
        .collect();
    assert_eq!(
        views,
        [
            ("Group A", "standings"),
            ("Showmatch", "list"),
            ("Matches", "list")
        ]
    );
    assert_eq!(stages[2]["id"], Value::Null);

    let group = &stages[0];
    let table: Vec<(&str, u64, u64)> = group["standings"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| {
            (
                r["team"]["name"].as_str().unwrap(),
                r["wins"].as_u64().unwrap(),
                r["losses"].as_u64().unwrap(),
            )
        })
        .collect();
    assert_eq!(table, [("Bravo", 1, 0), ("Alpha", 0, 0), ("Charlie", 0, 1)]);
    let m = group["matches"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["id"] == open)
        .unwrap();
    assert_eq!(m["round"], 2, "Bravo's second match");
    assert_eq!(m["pickable"], false, "not followed");
    assert_eq!(m["my_pick"], "none");
    assert_eq!(m["bracket"], Value::Null);

    // Followed: pickable.
    follow(&app, GUILD, ev).await;
    let body = get(&app, &token, &format!("/api/events/{}", ev.0)).await;
    assert_eq!(body["followed"], true);
    let m = body["stages"][0]["matches"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["id"] == open)
        .unwrap()
        .clone();
    assert_eq!(m["pickable"], true);
}

#[tokio::test]
async fn bracket_positions_are_derived_for_an_other_stage_with_feeds() {
    let app = auth_app(None).await;
    let ev = event(&app, "Cup", EventStatus::Running, None, None).await;
    let stage = store::events::insert_stage(
        &app.pool,
        &NewStage {
            event_id: ev,
            name: "Playoffs".into(),
            kind: StageKind::Other,
            position: 0,
        },
    )
    .await
    .unwrap();
    let add = |h: u32, bracket: BracketPos| {
        let pool = app.pool.clone();
        async move {
            let mut f = MatchFields::scheduled(ev, None, None, MatchFormat::Bo3, Some(at(h, 0)));
            f.stage_id = Some(stage);
            f.bracket = bracket;
            store::matches::insert(&pool, &f).await.unwrap()
        }
    };
    let sf1 = add(10, BracketPos::default()).await;
    let sf2 = add(11, BracketPos::default()).await;
    let feed = |m, outcome| {
        Some(domain::Feed {
            match_id: m,
            outcome,
        })
    };
    let fin = add(
        14,
        BracketPos {
            feeds_a_from: feed(sf1, domain::FeedOutcome::Winner),
            feeds_b_from: feed(sf2, domain::FeedOutcome::Winner),
            ..BracketPos::default()
        },
    )
    .await;
    let token = app.sign_in(ALICE, GUILD).await;
    let body = get(&app, &token, &format!("/api/events/{}", ev.0)).await;
    let stage = &body["stages"][0];
    assert_eq!(stage["view"], "bracket");
    let f = stage["matches"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["id"] == fin.0)
        .unwrap();
    assert_eq!(f["bracket"]["side"], "upper");
    assert_eq!(f["bracket"]["round"], 1);
    assert_eq!(f["bracket"]["derived"], true);
    assert_eq!(f["bracket"]["feeds_a"]["match_id"], sf1.0);
    assert_eq!(f["bracket"]["feeds_b"]["outcome"], "winner");
}

#[tokio::test]
async fn missing_events_sessions_and_revalidation() {
    let app = auth_app(None).await;
    let ev = event(&app, "Cup", EventStatus::Upcoming, None, None).await;
    follow(&app, GUILD, ev).await;
    let token = app.sign_in(ALICE, GUILD).await;
    let (status, body) = app.get("/api/events/999", &token).await;
    assert_eq!(
        (status, body["error"].as_str()),
        (StatusCode::NOT_FOUND, Some("not_found"))
    );
    for uri in ["/api/events", "/api/events/1"] {
        let (status, _) = send(&app.app, "GET", uri, &[], None).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{uri}");
    }
    let auth = format!("Bearer {token}");
    let req = |etag: Option<String>| {
        let mut r =
            Request::get(format!("/api/events/{}", ev.0)).header(header::AUTHORIZATION, &auth);
        if let Some(e) = etag {
            r = r.header(header::IF_NONE_MATCH, e);
        }
        app.app.clone().oneshot(r.body(Body::empty()).unwrap())
    };
    let res = req(None).await.unwrap();
    let etag = res.headers()[header::ETAG].to_str().unwrap().to_owned();
    let res = req(Some(etag)).await.unwrap();
    assert_eq!(res.status(), StatusCode::NOT_MODIFIED);
}

// ---- the fixture (acceptance criteria 1 and 2) ----------------------------------

struct Fixture {
    app: AuthApp,
    driver: Arc<FixtureDriver>,
    token: String,
    event: i64,
}

impl Fixture {
    async fn open() -> Self {
        use sources::fixture::FIXTURE;
        let app = auth_app(None).await;
        let rec = crate::test_util::reconciler(app.pool.clone(), &[FIXTURE]);
        let driver = Arc::new(
            FixtureDriver::open(app.pool.clone(), rec, Scenario::default_scenario(), t0())
                .await
                .unwrap(),
        );
        driver.sync().await.unwrap();
        let event: i64 = sqlx::query_scalar("SELECT id FROM events")
            .fetch_one(&app.pool)
            .await
            .unwrap();
        follow(&app, GUILD, EventId(event)).await;
        let token = app.sign_in(ALICE, GUILD).await;
        Self {
            app,
            driver,
            token,
            event,
        }
    }

    /// The canonical ID of a scenario match.
    async fn id(&self, ext: &str) -> i64 {
        sqlx::query_scalar(
            "SELECT entity_id FROM external_refs WHERE source = 'fixture' \
             AND entity_type = 'match' AND external_id = ?",
        )
        .bind(ext)
        .fetch_one(&self.app.pool)
        .await
        .unwrap()
    }

    async fn advance(&self, minutes: i64) {
        let (now, _) = self
            .driver
            .advance(Duration::minutes(minutes))
            .await
            .unwrap();
        self.app.clock.set(now);
    }

    async fn stage(&self, name: &str) -> Value {
        let body = get(
            &self.app,
            &self.token,
            &format!("/api/events/{}", self.event),
        )
        .await;
        body["stages"]
            .as_array()
            .unwrap()
            .iter()
            .find(|s| s["name"] == name)
            .unwrap_or_else(|| panic!("no stage {name}: {body}"))
            .clone()
    }
}

fn find(stage: &Value, id: i64) -> Value {
    stage["matches"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["id"] == id)
        .unwrap()
        .clone()
}

/// AC 1: the double-elimination playoff carries the scenario's feeds, and
/// a TBD slot fills when its feeder ends.
#[tokio::test]
async fn fixture_playoff_has_the_scenario_feeds_and_fills_tbd_slots() {
    let fx = Fixture::open().await;
    let [sf1, sf2, uf, l1, lf, gf] = [
        fx.id("ub-sf1").await,
        fx.id("ub-sf2").await,
        fx.id("ub-f").await,
        fx.id("lb-r1").await,
        fx.id("lb-f").await,
        fx.id("gf").await,
    ];
    let po = fx.stage("Playoffs").await;
    assert_eq!(po["kind"], "double_elim");
    assert_eq!(po["view"], "bracket");
    assert_eq!(po["matches"].as_array().unwrap().len(), 6);

    let pos = |id| {
        let b = find(&po, id)["bracket"].clone();
        (
            b["side"].as_str().unwrap().to_owned(),
            b["round"].as_u64().unwrap(),
            b["slot"].as_u64().unwrap(),
        )
    };
    let p = |s: &str, r, sl| (s.to_owned(), r, sl);
    assert_eq!(pos(sf1), p("upper", 0, 0));
    assert_eq!(pos(sf2), p("upper", 0, 1));
    assert_eq!(pos(uf), p("upper", 1, 0));
    assert_eq!(pos(l1), p("lower", 0, 0));
    assert_eq!(pos(lf), p("lower", 1, 0));
    assert_eq!(pos(gf), p("final", 2, 0));

    let feeds = |id| {
        let b = find(&po, id)["bracket"].clone();
        let f = |x: &Value| {
            (
                x["match_id"].as_i64().unwrap(),
                x["outcome"].as_str().unwrap().to_owned(),
            )
        };
        (f(&b["feeds_a"]), f(&b["feeds_b"]))
    };
    let w = |id| (id, "winner".to_owned());
    let l = |id| (id, "loser".to_owned());
    assert_eq!(feeds(uf), (w(sf1), w(sf2)));
    assert_eq!(feeds(l1), (l(sf1), l(sf2)));
    assert_eq!(feeds(lf), (l(uf), w(l1)));
    assert_eq!(feeds(gf), (w(uf), w(lf)));
    assert_eq!(find(&po, sf1)["bracket"]["feeds_a"], Value::Null);

    // Before the semis: the upper final has no teams yet.
    let m = find(&po, uf);
    assert_eq!(
        (m["team_a"].clone(), m["team_b"].clone()),
        (Value::Null, Value::Null)
    );

    // ub-sf1 (Bo3 at +480, two maps of 45 min) is over at +570; ub-sf2
    // plays a third map until +615.
    fx.advance(600).await;
    let po = fx.stage("Playoffs").await;
    assert_eq!(find(&po, sf1)["status"], "finished");
    let m = find(&po, uf);
    assert_eq!(m["team_a"]["acronym"], "AUR", "winner of ub-sf1");
    assert_eq!(m["team_b"], Value::Null, "ub-sf2 is still live");
    let m = find(&po, l1);
    assert_eq!(m["team_a"]["acronym"], "FJO", "loser of ub-sf1");

    fx.advance(30).await;
    let po = fx.stage("Playoffs").await;
    assert_eq!(find(&po, uf)["team_b"]["acronym"], "CIN");
    assert_eq!(find(&po, l1)["team_b"]["acronym"], "GLA");
}

/// AC 2: the Swiss stage's standings and rounds once it is over.
#[tokio::test]
async fn fixture_swiss_standings_and_rounds() {
    let fx = Fixture::open().await;
    let sw = fx.stage("Swiss Stage").await;
    assert_eq!(sw["view"], "standings");
    // Nothing played yet: every team at 0–0, by name.
    let table = |sw: &Value| -> Vec<(String, u64, u64, i64, i64)> {
        sw["standings"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| {
                (
                    r["team"]["acronym"].as_str().unwrap().to_owned(),
                    r["wins"].as_u64().unwrap(),
                    r["losses"].as_u64().unwrap(),
                    r["map_diff"].as_i64().unwrap(),
                    r["round_diff"].as_i64().unwrap(),
                )
            })
            .collect()
    };
    let teams: Vec<String> = table(&sw).into_iter().map(|r| r.0).collect();
    assert_eq!(
        teams,
        ["AUR", "BAS", "CIN", "DUN", "ECK", "FJO", "GLA", "HAL"]
    );

    // The Swiss stage ends with sw-r2-* (Bo3 at +180, at most three maps).
    fx.advance(180 + 3 * 45 + 5).await;
    let sw = fx.stage("Swiss Stage").await;
    let r = |t: &str, w, l, md, rd| (t.to_owned(), w, l, md, rd);
    assert_eq!(
        table(&sw),
        [
            r("CIN", 2, 0, 3, 13),
            r("AUR", 2, 0, 2, 11),
            r("GLA", 1, 1, 0, -2),
            r("DUN", 0, 0, 0, 0),
            r("ECK", 0, 0, 0, 0),
            r("FJO", 0, 1, -1, -2),
            r("BAS", 0, 1, -1, -5),
            r("HAL", 0, 2, -3, -15),
        ]
    );
    for (ext, round) in [
        ("sw-r1-1", 1),
        ("sw-r1-2", 1),
        ("sw-r1-3", 1),
        ("sw-r1-4", 1),
        ("sw-r2-1", 2),
        ("sw-r2-2", 2),
    ] {
        assert_eq!(find(&sw, fx.id(ext).await)["round"], round, "{ext}");
    }
}
