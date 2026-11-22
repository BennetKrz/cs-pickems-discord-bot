//! The leaderboard over HTTP (T14), on the fake Discord and a fake clock at
//! 09:00 on 2026-11-20. Picks are settled by the real settlement
//! (`sync::settle`), so the board shows what members would see.

use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use chrono::{DateTime, TimeZone, Utc};
use domain::{
    EventId, EventStatus, EventTier, MapPick, MapStatus, MatchFields, MatchFormat, MatchId,
    MatchMap, MatchStatus, NewEvent, NewTeam, PickParts, SeriesScore, TeamId,
};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use store::guilds::FollowedEvent;
use sync::RulesMode;
use tower::ServiceExt;

use crate::test_util::{ALICE, AuthApp, GUILD, OTHER_GUILD, OWNER, PICKER, auth_app, send};

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
    let team = |name: &'static str| async move {
        store::teams::insert(
            &app.pool,
            &NewTeam {
                name: name.into(),
                acronym: None,
                logo_url: None,
                country: None,
            },
        )
        .await
        .unwrap()
    };
    let a = team("Vitality").await;
    let b = team("NAVI").await;
    let ev = event(app, "IEM Test").await;
    follow(app, GUILD, ev).await;
    Seed { ev, a, b }
}

async fn bo3(app: &AuthApp, ev: EventId, s: &Seed) -> MatchId {
    store::matches::insert(
        &app.pool,
        &MatchFields::scheduled(ev, Some(s.a), Some(s.b), MatchFormat::Bo3, Some(at(10, 0))),
    )
    .await
    .unwrap()
}

async fn pick(app: &AuthApp, guild: &str, user: &str, m: MatchId, parts: PickParts) {
    store::picks::save(&app.pool, guild, user, m, &parts, at(9, 0))
        .await
        .unwrap();
}

fn series(winner: TeamId, a: u8, b: u8) -> PickParts {
    PickParts {
        winner_team_id: Some(winner),
        series: Some(SeriesScore { a, b }),
        ..PickParts::default()
    }
}

/// Sets the match result and settles it.
async fn finish(app: &AuthApp, m: MatchId, winner: TeamId, a: u8, b: u8) {
    let mut mm = store::matches::get(&app.pool, m).await.unwrap().unwrap();
    mm.fields.status = MatchStatus::Finished;
    mm.fields.started_at = Some(at(10, 0));
    mm.fields.ended_at = Some(at(12, 0));
    mm.fields.winner_team_id = Some(winner);
    mm.fields.maps_won_a = a;
    mm.fields.maps_won_b = b;
    store::matches::update(&app.pool, &mm).await.unwrap();
    settle(app, m).await;
}

async fn settle(app: &AuthApp, m: MatchId) {
    sync::settle::settle_match(&app.pool, m, at(12, 5), RulesMode::Pinned)
        .await
        .unwrap();
}

/// A GET with the session and an optional `If-None-Match`; returns the
/// status, the ETag and the JSON body (`Null` on a 304).
async fn get(
    app: &AuthApp,
    uri: &str,
    token: &str,
    etag: Option<&str>,
) -> (StatusCode, Option<String>, Value) {
    let mut req = Request::get(uri).header("authorization", format!("Bearer {token}"));
    if let Some(e) = etag {
        req = req.header(header::IF_NONE_MATCH, e);
    }
    let res = app
        .app
        .clone()
        .oneshot(req.body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = res.status();
    let tag = res
        .headers()
        .get(header::ETAG)
        .map(|v| v.to_str().unwrap().to_owned());
    let bytes = res.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        tag,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

async fn board(app: &AuthApp, token: &str, query: &str) -> Value {
    let (status, _, body) = get(app, &format!("/api/leaderboard{query}"), token, None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body
}

/// `(user_id, rank, points)` per row.
fn summary(body: &Value) -> Vec<(String, u64, i64)> {
    body["rows"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| {
            (
                r["user_id"].as_str().unwrap().to_owned(),
                r["rank"].as_u64().unwrap(),
                r["points"].as_i64().unwrap(),
            )
        })
        .collect()
}

/// Signs in Alice, Picker and the owner (so their users exist); returns
/// Alice's token.
async fn sign_in_all(app: &AuthApp) -> String {
    app.sign_in(PICKER, GUILD).await;
    app.sign_in(OWNER, GUILD).await;
    app.sign_in(ALICE, GUILD).await
}

#[tokio::test]
async fn needs_a_session_and_a_numeric_event_id() {
    let app = auth_app(None).await;
    let (status, body) = send(&app.app, "GET", "/api/leaderboard", &[], None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(body["error"], "unauthenticated");

    let token = app.sign_in(ALICE, GUILD).await;
    let (status, _, body) = get(&app, "/api/leaderboard?event_id=abc", &token, None).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(body["error"], "invalid_query");

    // No picks yet: an empty board, no events followed.
    let body = board(&app, &token, "").await;
    assert_eq!(
        body,
        json!({
            "event_id": null, "events": [], "match_day": null,
            "has_previous": false, "live": false, "rows": [],
        })
    );
}

/// AC 2: a settlement change is in the next response, so the Activity sees
/// it within one poll interval; the ETag changes with it.
#[tokio::test]
async fn a_settlement_change_shows_up_in_the_next_response() {
    let app = auth_app(None).await;
    let s = seed(&app).await;
    let token = sign_in_all(&app).await;
    let m = bo3(&app, s.ev, &s).await;
    pick(&app, GUILD, ALICE, m, series(s.a, 2, 0)).await;
    pick(&app, GUILD, PICKER, m, series(s.b, 1, 2)).await;

    finish(&app, m, s.a, 2, 0).await;
    let (status, tag, body) = get(&app, "/api/leaderboard", &token, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        summary(&body),
        [(ALICE.to_owned(), 1, 6), (PICKER.to_owned(), 2, 0)]
    );
    let tag = tag.unwrap();
    // Unchanged: a 304.
    let (status, _, _) = get(&app, "/api/leaderboard", &token, Some(&tag)).await;
    assert_eq!(status, StatusCode::NOT_MODIFIED);

    // A correction: NAVI won 2:1 after all.
    finish(&app, m, s.b, 1, 2).await;
    let (status, new_tag, body) = get(&app, "/api/leaderboard", &token, Some(&tag)).await;
    assert_eq!(status, StatusCode::OK);
    assert_ne!(new_tag.unwrap(), tag);
    assert_eq!(
        summary(&body),
        [(PICKER.to_owned(), 1, 6), (ALICE.to_owned(), 2, 0)]
    );
    let picker = &body["rows"][0];
    assert_eq!(picker["winner_accuracy"], 1.0);
    assert_eq!(picker["series_hits"], 1);
    assert_eq!(body["rows"][1]["winner_accuracy"], 0.0);
}

#[tokio::test]
async fn live_points_count_and_are_marked_provisional() {
    let app = auth_app(None).await;
    let s = seed(&app).await;
    let token = sign_in_all(&app).await;
    let m = bo3(&app, s.ev, &s).await;
    // The match winner and map 1 with its score. Rounds are relative to
    // team A/B: 13:7 means A won.
    let map1 = |winner, rounds| PickParts {
        winner_team_id: Some(winner),
        maps: vec![MapPick {
            map_number: 1,
            winner_team_id: winner,
            rounds: Some(rounds),
            overtime: false,
        }],
        ..PickParts::default()
    };
    pick(&app, GUILD, ALICE, m, map1(s.a, (13, 7))).await;
    pick(&app, GUILD, PICKER, m, map1(s.b, (7, 13))).await;

    // Live, map 1 to Vitality 13:7.
    let mut mm = store::matches::get(&app.pool, m).await.unwrap().unwrap();
    mm.fields.status = MatchStatus::Live;
    mm.fields.started_at = Some(at(10, 0));
    mm.fields.maps_won_a = 1;
    store::matches::update(&app.pool, &mm).await.unwrap();
    store::matches::upsert_map(
        &app.pool,
        &MatchMap {
            match_id: m,
            number: 1,
            map_name: Some("Inferno".into()),
            status: MapStatus::Finished,
            rounds_a: Some(13),
            rounds_b: Some(7),
            winner_team_id: Some(s.a),
        },
    )
    .await
    .unwrap();
    settle(&app, m).await;

    let body = board(&app, &token, "").await;
    assert_eq!(body["live"], true);
    let alice = &body["rows"][0];
    assert_eq!(alice["user_id"], ALICE);
    assert_eq!(alice["points"], 3);
    assert_eq!(alice["provisional_points"], 3);
    assert_eq!(alice["provisional_picks"], 1);
    assert_eq!(alice["map_hits"], 1);
    // The winner part is still pending: no accuracy yet.
    assert_eq!(alice["winner_decided"], 0);
    assert_eq!(alice["winner_accuracy"], Value::Null);
    let picker = &body["rows"][1];
    assert_eq!(
        (
            picker["points"].clone(),
            picker["provisional_picks"].clone()
        ),
        (json!(0), json!(1))
    );

    finish(&app, m, s.a, 2, 0).await;
    let body = board(&app, &token, "").await;
    assert_eq!(body["live"], false);
    // Winner 3, map winner 1, map score 2.
    assert_eq!(body["rows"][0]["points"], 6);
    assert_eq!(body["rows"][0]["provisional_points"], 0);
}

#[tokio::test]
async fn filters_by_event_keeps_guilds_apart_and_names_members() {
    let app = auth_app(None).await;
    let s = seed(&app).await;
    let token = sign_in_all(&app).await;
    let other_token = app.sign_in(ALICE, OTHER_GUILD).await;
    let ev2 = event(&app, "Major").await;
    follow(&app, GUILD, ev2).await;
    let m1 = bo3(&app, s.ev, &s).await;
    let m2 = bo3(&app, ev2, &s).await;
    pick(&app, GUILD, ALICE, m1, series(s.a, 2, 0)).await;
    pick(&app, GUILD, PICKER, m2, series(s.a, 2, 1)).await;
    pick(&app, OTHER_GUILD, ALICE, m2, series(s.a, 2, 1)).await;
    finish(&app, m1, s.a, 2, 0).await;
    finish(&app, m2, s.a, 2, 1).await;

    let all = board(&app, &token, "").await;
    assert_eq!(
        summary(&all),
        [(ALICE.to_owned(), 1, 6), (PICKER.to_owned(), 1, 6)]
    );
    assert_eq!(
        all["events"],
        json!([{ "id": s.ev.0, "name": "IEM Test" }, { "id": ev2.0, "name": "Major" }])
    );
    let alice = &all["rows"][0];
    assert_eq!(alice["display_name"], "ali");
    assert_eq!(
        alice["avatar_url"],
        format!("/api/img/avatar/{ALICE}?v=abc123")
    );
    assert_eq!(all["rows"][1]["display_name"], "picker");

    let one = board(&app, &token, &format!("?event_id={}", ev2.0)).await;
    assert_eq!(one["event_id"], ev2.0);
    assert_eq!(summary(&one), [(PICKER.to_owned(), 1, 6)]);
    let none = board(&app, &token, "?event_id=999").await;
    assert_eq!(none["rows"], json!([]));

    // The other guild sees only its own pick, and follows nothing.
    let other = board(&app, &other_token, "").await;
    assert_eq!(summary(&other), [(ALICE.to_owned(), 1, 6)]);
    assert_eq!(other["events"], json!([]));
}
