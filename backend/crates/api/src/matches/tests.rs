//! Match list and match detail over HTTP (T12), on the fake Discord and a
//! fake clock at 09:00 on 2026-11-20.

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use chrono::{DateTime, Duration, TimeZone, Utc};
use domain::{
    EventId, EventStatus, EventTier, MapStatus, MatchFields, MatchFormat, MatchId, MatchMap,
    MatchStatus, NewEvent, NewStage, NewTeam, PickParts, SeriesScore, StageKind, TeamId,
};
use serde_json::{Value, json};
use store::guilds::FollowedEvent;
use tower::ServiceExt;

use crate::SERVER_TIME;
use crate::test_util::{ALICE, AuthApp, GUILD, OTHER_GUILD, OWNER, PICKER, auth_app, send, t0};

fn at(h: u32, m: u32) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 11, 20, h, m, 0).unwrap()
}

struct Seed {
    ev: EventId,
    a: TeamId,
    b: TeamId,
}

async fn event(app: &AuthApp, name: &str) -> EventId {
    store::events::insert(
        &app.pool,
        &NewEvent {
            name: name.into(),
            tier: EventTier::S,
            begin_at: None,
            end_at: None,
            status: EventStatus::Running,
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

async fn seed(app: &AuthApp) -> Seed {
    let team = |name: &'static str, logo: Option<&'static str>| async move {
        store::teams::insert(
            &app.pool,
            &NewTeam {
                name: name.into(),
                acronym: Some(name[..3].to_uppercase()),
                logo_url: logo.map(str::to_owned),
                country: None,
            },
        )
        .await
        .unwrap()
    };
    let a = team("Vitality", Some("https://cdn-api.pandascore.co/v.png")).await;
    let b = team("NAVI", None).await;
    let ev = event(app, "IEM Test").await;
    follow(app, GUILD, ev).await;
    Seed { ev, a, b }
}

/// Inserts a Bo3 of `s.ev` between A and B, scheduled at `scheduled`, then
/// applies `f` to its fields.
async fn add_match(
    app: &AuthApp,
    s: &Seed,
    scheduled: Option<DateTime<Utc>>,
    f: impl FnOnce(&mut MatchFields),
) -> i64 {
    let mut fields =
        MatchFields::scheduled(s.ev, Some(s.a), Some(s.b), MatchFormat::Bo3, scheduled);
    f(&mut fields);
    store::matches::insert(&app.pool, &fields).await.unwrap().0
}

async fn list(app: &AuthApp, token: &str, query: &str) -> Value {
    let (status, body) = app.get(&format!("/api/matches?{query}"), token).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body
}

fn ids(body: &Value) -> Vec<i64> {
    body["matches"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["id"].as_i64().unwrap())
        .collect()
}

async fn setup() -> (AuthApp, Seed, String) {
    let app = auth_app(None).await;
    let s = seed(&app).await;
    let token = app.sign_in(ALICE, GUILD).await;
    (app, s, token)
}

#[tokio::test]
async fn segments_follow_status_and_the_server_clock() {
    let (app, s, token) = setup().await;
    let soon = add_match(&app, &s, Some(at(18, 0)), |_| {}).await;
    let in_13_days = add_match(&app, &s, Some(t0() + Duration::days(13)), |_| {}).await;
    let in_15_days = add_match(&app, &s, Some(t0() + Duration::days(15)), |_| {}).await;
    let overdue = add_match(&app, &s, Some(at(8, 30)), |_| {}).await;
    let live = add_match(&app, &s, Some(at(8, 0)), |f| {
        f.status = MatchStatus::Live;
        f.started_at = Some(at(8, 2));
        f.maps_won_a = 1;
    })
    .await;
    let done = add_match(&app, &s, Some(at(5, 0)), |f| {
        f.status = MatchStatus::Finished;
        f.ended_at = Some(at(7, 0));
        f.maps_won_a = 2;
        f.winner_team_id = Some(s.a);
    })
    .await;
    let old = add_match(&app, &s, Some(t0() - Duration::days(31)), |f| {
        f.status = MatchStatus::Finished;
        f.ended_at = Some(t0() - Duration::days(31));
    })
    .await;
    let canceled = add_match(&app, &s, Some(at(6, 0)), |f| {
        f.status = MatchStatus::Canceled;
    })
    .await;

    let up = list(&app, &token, "state=upcoming").await;
    assert_eq!(up["state"], "upcoming");
    assert_eq!(up["following_any"], true);
    assert_eq!(ids(&up), vec![overdue, soon, in_13_days]);
    assert!(!ids(&up).contains(&in_15_days));
    // The overdue match is locked by the clock but not live yet.
    let row = &up["matches"][0];
    assert_eq!(
        (row["locked"].clone(), row["pickable"].clone()),
        (json!(true), json!(false))
    );
    let row = &up["matches"][1];
    assert_eq!(row["pickable"], true);
    assert_eq!(row["lock_at"], json!(at(18, 0)));
    assert_eq!(row["event"], json!({ "id": s.ev.0, "name": "IEM Test" }));
    assert_eq!(row["team_a"]["name"], "Vitality");
    assert_eq!(row["team_a"]["acronym"], "VIT");
    assert!(
        row["team_a"]["logo_url"]
            .as_str()
            .unwrap()
            .starts_with(&format!("/api/img/team/{}?v=", s.a.0))
    );
    assert_eq!(row["team_b"]["logo_url"], Value::Null);
    assert_eq!(row["my_pick"], "none");
    assert_eq!(row["next_cursor"], Value::Null);

    let lv = list(&app, &token, "state=live").await;
    assert_eq!(ids(&lv), vec![live]);
    assert_eq!(lv["matches"][0]["maps_won_a"], 1);

    let fin = list(&app, &token, "state=finished").await;
    assert_eq!(ids(&fin), vec![done, canceled]);
    assert!(!ids(&fin).contains(&old));

    // Thirteen days later the far match is in the window.
    app.clock.set(t0() + Duration::days(2));
    let up = list(&app, &token, "state=upcoming").await;
    assert_eq!(ids(&up), vec![overdue, soon, in_13_days, in_15_days]);
}

#[tokio::test]
async fn a_match_moves_from_upcoming_to_live_to_results() {
    let (app, s, token) = setup().await;
    let id = add_match(&app, &s, Some(at(18, 0)), |_| {}).await;
    assert_eq!(ids(&list(&app, &token, "state=upcoming").await), vec![id]);

    let mut m = store::matches::get(&app.pool, MatchId(id))
        .await
        .unwrap()
        .unwrap();
    m.fields.status = MatchStatus::Live;
    m.fields.started_at = Some(at(18, 1));
    m.fields.maps_won_a = 1;
    store::matches::update(&app.pool, &m).await.unwrap();
    for (n, status, rounds) in [
        (1, MapStatus::Finished, Some((13, 5))),
        (2, MapStatus::Live, Some((7, 4))),
    ] {
        store::matches::upsert_map(
            &app.pool,
            &MatchMap {
                match_id: MatchId(id),
                number: n,
                map_name: Some(format!("map{n}")),
                status,
                rounds_a: rounds.map(|r| r.0),
                rounds_b: rounds.map(|r| r.1),
                winner_team_id: (status == MapStatus::Finished).then_some(s.a),
            },
        )
        .await
        .unwrap();
    }
    app.clock.set(at(18, 30));
    assert!(ids(&list(&app, &token, "state=upcoming").await).is_empty());
    let lv = list(&app, &token, "state=live").await;
    assert_eq!(ids(&lv), vec![id]);
    assert_eq!(
        lv["matches"][0]["current_map"],
        json!({ "number": 2, "map_name": "map2", "rounds_a": 7, "rounds_b": 4 })
    );

    m.fields.status = MatchStatus::Finished;
    m.fields.ended_at = Some(at(20, 0));
    m.fields.maps_won_a = 2;
    m.fields.winner_team_id = Some(s.a);
    store::matches::update(&app.pool, &m).await.unwrap();
    app.clock.set(at(20, 5));
    assert!(ids(&list(&app, &token, "state=live").await).is_empty());
    assert_eq!(ids(&list(&app, &token, "state=finished").await), vec![id]);
}

#[tokio::test]
async fn only_followed_events_and_own_guild_picks() {
    let (app, s, token) = setup().await;
    let followed = add_match(&app, &s, Some(at(18, 0)), |_| {}).await;
    let other_ev = event(&app, "Elsewhere").await;
    follow(&app, OTHER_GUILD, other_ev).await;
    let mut f = MatchFields::scheduled(
        other_ev,
        Some(s.a),
        Some(s.b),
        MatchFormat::Bo1,
        Some(at(17, 0)),
    );
    f.stage_id = None;
    let unfollowed = store::matches::insert(&app.pool, &f).await.unwrap().0;

    let winner = PickParts {
        winner_team_id: Some(s.a),
        ..PickParts::default()
    };
    for (guild, user) in [(GUILD, PICKER), (GUILD, OWNER), (OTHER_GUILD, ALICE)] {
        store::users::upsert_user(
            &app.pool,
            &store::users::User {
                discord_user_id: user.into(),
                username: format!("u{user}"),
                global_name: None,
                avatar_hash: None,
                updated_at: at(8, 0),
            },
        )
        .await
        .unwrap();
        store::picks::save(&app.pool, guild, user, MatchId(followed), &winner, at(8, 0))
            .await
            .unwrap();
    }

    let up = list(&app, &token, "state=upcoming").await;
    assert_eq!(ids(&up), vec![followed]);
    assert_eq!(up["matches"][0]["pick_count"], 2);
    assert_eq!(
        up["matches"][0]["my_pick"], "none",
        "Alice picked in the other guild only"
    );

    // The other guild sees its own event, and its single pick.
    let theirs = app.sign_in(ALICE, OTHER_GUILD).await;
    let up = list(&app, &theirs, "state=upcoming").await;
    assert_eq!(ids(&up), vec![unfollowed]);

    // A guild that follows nothing gets an empty list with `following_any`.
    store::guilds::unfollow_event(&app.pool, GUILD, s.ev)
        .await
        .unwrap();
    let up = list(&app, &token, "state=upcoming").await;
    assert_eq!(up["following_any"], false);
    assert!(ids(&up).is_empty());
}

#[tokio::test]
async fn my_pick_status_and_points() {
    let (app, s, token) = setup().await;
    let partial = add_match(&app, &s, Some(at(18, 0)), |_| {}).await;
    let full = add_match(&app, &s, Some(at(19, 0)), |_| {}).await;
    let done = add_match(&app, &s, Some(at(5, 0)), |f| {
        f.status = MatchStatus::Finished;
        f.ended_at = Some(at(7, 0));
    })
    .await;
    let map = |n: u8| domain::MapPick {
        map_number: n,
        winner_team_id: s.a,
        rounds: None,
        overtime: false,
    };
    let save = |m: i64, parts: PickParts| {
        let pool = app.pool.clone();
        async move {
            store::picks::save(&pool, GUILD, ALICE, MatchId(m), &parts, at(8, 0))
                .await
                .unwrap()
        }
    };
    save(
        partial,
        PickParts {
            winner_team_id: Some(s.a),
            series: Some(SeriesScore { a: 2, b: 0 }),
            maps: vec![map(1)],
        },
    )
    .await;
    save(
        full,
        PickParts {
            winner_team_id: Some(s.a),
            series: Some(SeriesScore { a: 2, b: 0 }),
            maps: vec![map(1), map(2)],
        },
    )
    .await;
    let p = save(
        done,
        PickParts {
            winner_team_id: Some(s.a),
            ..PickParts::default()
        },
    )
    .await;
    store::picks::upsert_result(
        &app.pool,
        &store::picks::PickResult {
            pick_id: p.id,
            points: 3,
            breakdown: json!({ "winner": { "outcome": "hit", "points": 3 }, "series": null, "maps": [] }),
            rules_version: 1,
            settled_at: at(7, 5),
            state: domain::ResultState::Final,
            pending_parts: 0,
        },
    )
    .await
    .unwrap();

    let up = list(&app, &token, "state=upcoming").await;
    let status: Vec<_> = up["matches"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["my_pick"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(status, vec!["partial", "full"]);
    assert_eq!(up["matches"][0]["my_result"], Value::Null);

    let fin = list(&app, &token, "state=finished").await;
    assert_eq!(fin["matches"][0]["my_pick"], "partial");
    assert_eq!(
        fin["matches"][0]["my_result"],
        json!({ "points": 3, "state": "final", "pending_parts": 0 })
    );

    // Everyone's picks carry the result with its breakdown.
    let (status, picks) = app.get(&format!("/api/matches/{done}/picks"), &token).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(picks["picks"][0]["result"]["points"], 3);
    assert_eq!(picks["picks"][0]["result"]["state"], "final");
    assert_eq!(
        picks["picks"][0]["result"]["breakdown"]["winner"]["outcome"],
        "hit"
    );
    let (_, picks) = app.get(&format!("/api/matches/{full}/picks"), &token).await;
    assert_eq!(picks["picks"][0]["result"], Value::Null);
}

#[tokio::test]
async fn results_page_with_a_cursor_and_filter_by_event() {
    let (app, s, token) = setup().await;
    let other_ev = event(&app, "Second Cup").await;
    follow(&app, GUILD, other_ev).await;
    let mut done = Vec::new();
    for i in 0..(crate::matches::FINISHED_PAGE + 5) {
        let end = t0() - Duration::hours(i + 1);
        done.push(
            add_match(&app, &s, Some(end - Duration::hours(1)), |f| {
                f.status = MatchStatus::Finished;
                f.ended_at = Some(end);
            })
            .await,
        );
    }
    let mut f = MatchFields::scheduled(
        other_ev,
        Some(s.a),
        Some(s.b),
        MatchFormat::Bo1,
        Some(at(2, 0)),
    );
    f.status = MatchStatus::Finished;
    f.ended_at = Some(t0() - Duration::minutes(30));
    let in_other = store::matches::insert(&app.pool, &f).await.unwrap().0;

    let page1 = list(&app, &token, &format!("state=finished&event_id={}", s.ev.0)).await;
    assert_eq!(ids(&page1), done[..30].to_vec());
    let cursor = page1["next_cursor"].as_str().unwrap().to_owned();
    let page2 = list(
        &app,
        &token,
        &format!("state=finished&event_id={}&cursor={cursor}", s.ev.0),
    )
    .await;
    assert_eq!(ids(&page2), done[30..].to_vec());
    assert_eq!(page2["next_cursor"], Value::Null);

    let other = list(
        &app,
        &token,
        &format!("state=finished&event_id={}", other_ev.0),
    )
    .await;
    assert_eq!(ids(&other), vec![in_other]);
}

#[tokio::test]
async fn bad_queries_and_missing_sessions() {
    let (app, _s, token) = setup().await;
    for q in [
        "",
        "state=past",
        "state=upcoming&event_id=x",
        "state=finished&cursor=nope",
        "state=finished&cursor=1_x",
    ] {
        let (status, body) = app.get(&format!("/api/matches?{q}"), &token).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{q}: {body}");
        assert_eq!(body["error"], "invalid_query", "{q}");
    }
    for uri in ["/api/matches?state=live", "/api/matches/1"] {
        let (status, _) = send(&app.app, "GET", uri, &[], None).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{uri}");
    }
    let (status, body) = app.get("/api/matches/999", &token).await;
    assert_eq!(
        (status, body["error"].as_str()),
        (StatusCode::NOT_FOUND, Some("not_found"))
    );
}

#[tokio::test]
async fn detail_has_header_and_maps_also_for_unfollowed_events() {
    let (app, s, token) = setup().await;
    let stage = store::events::insert_stage(
        &app.pool,
        &NewStage {
            event_id: s.ev,
            name: "Playoffs".into(),
            kind: StageKind::SingleElim,
            position: 1,
        },
    )
    .await
    .unwrap();
    let id = add_match(&app, &s, Some(at(10, 0)), |f| {
        f.stage_id = Some(stage);
        f.status = MatchStatus::Finished;
        f.started_at = Some(at(10, 0));
        f.ended_at = Some(at(12, 0));
        f.maps_won_a = 2;
        f.winner_team_id = Some(s.a);
    })
    .await;
    for (n, status, rounds, winner) in [
        (1, MapStatus::Finished, Some((13, 9)), Some(s.a)),
        (2, MapStatus::Finished, Some((16, 14)), Some(s.a)),
        (3, MapStatus::NotPlayed, None, None),
    ] {
        store::matches::upsert_map(
            &app.pool,
            &MatchMap {
                match_id: MatchId(id),
                number: n,
                map_name: (n < 3).then(|| format!("map{n}")),
                status,
                rounds_a: rounds.map(|r| r.0),
                rounds_b: rounds.map(|r| r.1),
                winner_team_id: winner,
            },
        )
        .await
        .unwrap();
    }
    app.clock.set(at(13, 0));
    let (status, body) = app.get(&format!("/api/matches/{id}"), &token).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["stage"], json!({ "id": stage.0, "name": "Playoffs" }));
    assert_eq!(body["status"], "finished");
    assert_eq!(body["format"], "bo3");
    assert_eq!(body["winner_team_id"], s.a.0);
    assert_eq!(
        (body["locked"].clone(), body["pickable"].clone()),
        (json!(true), json!(false))
    );
    assert_eq!(body["current_map"], Value::Null);
    assert_eq!(body["maps"].as_array().unwrap().len(), 3);
    assert_eq!(
        body["maps"][1],
        json!({ "number": 2, "map_name": "map2", "status": "finished", "rounds_a": 16, "rounds_b": 14, "winner_team_id": s.a.0 })
    );
    assert_eq!(body["maps"][2]["status"], "not_played");
    assert_eq!(body["maps"][2]["map_name"], Value::Null);

    // An upcoming match with a TBD team in an unfollowed event still opens.
    store::guilds::unfollow_event(&app.pool, GUILD, s.ev)
        .await
        .unwrap();
    let tbd = add_match(&app, &s, Some(at(20, 0)), |f| f.team_b_id = None).await;
    let (status, body) = app.get(&format!("/api/matches/{tbd}"), &token).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["team_b"], Value::Null);
    assert_eq!(body["pickable"], false);
    assert_eq!(body["locked"], false);
    assert_eq!(body["maps"], json!([]));
}

#[tokio::test]
async fn polls_revalidate_and_carry_the_server_time() {
    let (app, s, token) = setup().await;
    add_match(&app, &s, Some(at(18, 0)), |_| {}).await;
    let auth = format!("Bearer {token}");
    let get = |etag: Option<String>| {
        let mut req =
            Request::get("/api/matches?state=upcoming").header(header::AUTHORIZATION, &auth);
        if let Some(e) = etag {
            req = req.header(header::IF_NONE_MATCH, e);
        }
        app.app.clone().oneshot(req.body(Body::empty()).unwrap())
    };
    let res = get(None).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(
        res.headers()[SERVER_TIME].to_str().unwrap(),
        "2026-11-20T09:00:00.000Z"
    );
    let etag = res.headers()[header::ETAG].to_str().unwrap().to_owned();

    // The body has no "now", so a later poll is still a 304, with the new time.
    app.clock.set(at(9, 0) + Duration::seconds(20));
    let res = get(Some(etag)).await.unwrap();
    assert_eq!(res.status(), StatusCode::NOT_MODIFIED);
    assert_eq!(
        res.headers()[SERVER_TIME].to_str().unwrap(),
        "2026-11-20T09:00:20.000Z"
    );

    // Errors carry it too.
    let res = app
        .app
        .clone()
        .oneshot(
            Request::get("/api/matches?state=live")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
    assert!(res.headers().contains_key(SERVER_TIME));
}

/// The acceptance criterion's fixture lifecycle: the fixture source on its
/// fake clock drives a match through the list's segments.
#[tokio::test]
async fn fixture_lifecycle_moves_a_match_through_the_segments() {
    use crate::dev::{FixtureDriver, Scenario};
    use sources::fixture::FIXTURE;

    let app = auth_app(None).await;
    let rec = crate::test_util::reconciler(app.pool.clone(), &[FIXTURE]);
    let d = Arc::new(
        FixtureDriver::open(app.pool.clone(), rec, Scenario::default_scenario(), t0())
            .await
            .unwrap(),
    );
    d.sync().await.unwrap();
    let ev: i64 = sqlx::query_scalar("SELECT id FROM events")
        .fetch_one(&app.pool)
        .await
        .unwrap();
    follow(&app, GUILD, EventId(ev)).await;
    let token = app.sign_in(ALICE, GUILD).await;

    // `sw-r2-1`: a Bo3 at anchor + 180 min, three maps of 45 min.
    let id: i64 = sqlx::query_scalar(
        "SELECT entity_id FROM external_refs WHERE source = ? AND entity_type = 'match' \
         AND external_id = 'sw-r2-1'",
    )
    .bind(FIXTURE)
    .fetch_one(&app.pool)
    .await
    .unwrap();
    let step = |minutes: i64| {
        let d = d.clone();
        let clock = app.clock.clone();
        async move {
            let (now, _) = d.advance(Duration::minutes(minutes)).await.unwrap();
            clock.set(now);
        }
    };

    let up = list(&app, &token, "state=upcoming").await;
    assert!(ids(&up).contains(&id));
    let row = up["matches"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["id"] == id)
        .unwrap();
    assert_eq!(row["pickable"], true);

    step(180 + 50).await; // map 1 done, map 2 live
    assert!(!ids(&list(&app, &token, "state=upcoming").await).contains(&id));
    let lv = list(&app, &token, "state=live").await;
    let row = lv["matches"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["id"] == id)
        .unwrap();
    assert_eq!(
        row["maps_won_a"].as_u64().unwrap() + row["maps_won_b"].as_u64().unwrap(),
        1
    );
    assert_eq!(row["current_map"]["number"], 2);
    assert_eq!(row["locked"], true);

    step(100).await; // all three maps done
    assert!(!ids(&list(&app, &token, "state=live").await).contains(&id));
    let fin = list(&app, &token, "state=finished").await;
    let row = fin["matches"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["id"] == id)
        .unwrap();
    assert_eq!(row["status"], "finished");
    assert_eq!(
        (row["maps_won_a"].clone(), row["maps_won_b"].clone()),
        (json!(2), json!(1))
    );
}

/// T09, AC 1: with the fixture source, `/event search` finds the event and
/// following the chosen result makes its matches appear in the list the
/// Activity polls (every 60 s when idle), pickable where teams are known.
#[tokio::test]
async fn following_the_fixture_event_from_search_fills_the_list() {
    use crate::dev::{FixtureDriver, Scenario};
    use sources::fixture::FIXTURE;
    use sync::{Scheduler, SchedulerParts, SourceEntry, SyncConfig};

    let app = auth_app(None).await;
    let rec = crate::test_util::reconciler(app.pool.clone(), &[FIXTURE]);
    let d = Arc::new(
        FixtureDriver::open(
            app.pool.clone(),
            rec.clone(),
            Scenario::default_scenario(),
            t0(),
        )
        .await
        .unwrap(),
    );
    d.sync().await.unwrap();
    // As the app wires it in dev: the fixture source answers searches only.
    let (_scheduler, handle) = Scheduler::open(SchedulerParts {
        reconciler: rec.clone(),
        clock: app.clock.clone(),
        config: SyncConfig::default(),
        sources: vec![SourceEntry {
            adapter: d.adapter(),
            budget: None,
            search_only: true,
        }],
        seed: Some(1),
    })
    .await
    .unwrap();
    let token = app.sign_in(ALICE, GUILD).await;
    assert!(ids(&list(&app, &token, "state=upcoming").await).is_empty());

    let name = d.adapter().event().name;
    let query = name.split_whitespace().next().unwrap();
    let out = handle.search(query).await;
    assert_eq!(out.hits.len(), 1, "{out:?}");
    assert_eq!(out.hits[0].source, FIXTURE);
    let followed = sync::follow::follow_hit(&rec, Some(&handle), GUILD, OWNER, &out.hits[0], t0())
        .await
        .unwrap();
    assert!(followed.newly_followed);
    assert_eq!(followed.event.name, name);

    let up = list(&app, &token, "state=upcoming").await;
    let rows = up["matches"].as_array().unwrap();
    assert!(!rows.is_empty(), "{up}");
    assert!(rows.iter().all(|m| m["event"]["name"] == name), "{up}");
    assert!(rows.iter().any(|m| m["pickable"] == true), "{up}");
    // Another guild still sees nothing.
    let theirs = app.sign_in(ALICE, OTHER_GUILD).await;
    assert!(ids(&list(&app, &theirs, "state=upcoming").await).is_empty());
}
