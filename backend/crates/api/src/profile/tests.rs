//! Profiles and pick history over HTTP (T15), on the fake Discord and a
//! fake clock at 09:00 on 2026-11-20. Picks are settled by the real
//! settlement (`sync::settle`), as in the leaderboard tests.

use axum::http::StatusCode;
use chrono::{DateTime, Duration, TimeZone, Utc};
use domain::{
    EventId, EventStatus, EventTier, MapPick, MapStatus, MatchFields, MatchFormat, MatchId,
    MatchMap, MatchStatus, NewEvent, NewTeam, PickParts, SeriesScore, TeamId,
};
use serde_json::{Value, json};
use store::guilds::FollowedEvent;
use sync::RulesMode;

use super::SETTLED_PAGE;
use crate::test_util::{
    ALICE, AuthApp, GUILD, OTHER_GUILD, OWNER, PICKER, STRANGER, auth_app, send,
};

fn at(h: u32, m: u32) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 11, 20, h, m, 0).unwrap()
}

struct Seed {
    ev: EventId,
    ev2: EventId,
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

async fn follow(app: &AuthApp, ev: EventId) {
    store::guilds::follow_event(
        &app.pool,
        &FollowedEvent {
            guild_id: GUILD.into(),
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
    let ev2 = event(app, "Major").await;
    follow(app, ev).await;
    follow(app, ev2).await;
    Seed { ev, ev2, a, b }
}

/// A Bo3 at `h`:00, locking then.
async fn bo3(app: &AuthApp, ev: EventId, s: &Seed, h: u32) -> MatchId {
    store::matches::insert(
        &app.pool,
        &MatchFields::scheduled(ev, Some(s.a), Some(s.b), MatchFormat::Bo3, Some(at(h, 0))),
    )
    .await
    .unwrap()
}

async fn pick(app: &AuthApp, guild: &str, user: &str, m: MatchId, parts: PickParts) {
    store::picks::save(&app.pool, guild, user, m, &parts, at(9, 0))
        .await
        .unwrap();
}

/// A winner, a series score and map 1 with an exact score. Scores are
/// relative to team A/B, so the map winner follows from the rounds.
fn full(s: &Seed, winner: TeamId, a: u8, b: u8, map1: (u16, u16)) -> PickParts {
    PickParts {
        winner_team_id: Some(winner),
        series: Some(SeriesScore { a, b }),
        maps: vec![MapPick {
            map_number: 1,
            winner_team_id: if map1.0 > map1.1 { s.a } else { s.b },
            rounds: Some(map1),
            overtime: false,
        }],
    }
}

fn winner_only(winner: TeamId) -> PickParts {
    PickParts {
        winner_team_id: Some(winner),
        ..PickParts::default()
    }
}

async fn update(app: &AuthApp, m: MatchId, f: impl FnOnce(&mut MatchFields)) {
    let mut mm = store::matches::get(&app.pool, m).await.unwrap().unwrap();
    f(&mut mm.fields);
    store::matches::update(&app.pool, &mm).await.unwrap();
}

async fn map1(app: &AuthApp, m: MatchId, s: &Seed, rounds: (u16, u16)) {
    store::matches::upsert_map(
        &app.pool,
        &MatchMap {
            match_id: m,
            number: 1,
            map_name: Some("Inferno".into()),
            status: MapStatus::Finished,
            rounds_a: Some(rounds.0),
            rounds_b: Some(rounds.1),
            winner_team_id: Some(if rounds.0 > rounds.1 { s.a } else { s.b }),
        },
    )
    .await
    .unwrap();
}

async fn settle(app: &AuthApp, m: MatchId) {
    sync::settle::settle_match(&app.pool, m, at(23, 0), RulesMode::Pinned)
        .await
        .unwrap();
}

/// Sets the result, `end` hours after 10:00, and settles.
async fn finish(app: &AuthApp, m: MatchId, winner: TeamId, a: u8, b: u8, end: i64) {
    update(app, m, |f| {
        f.status = MatchStatus::Finished;
        f.started_at = Some(at(10, 0));
        f.ended_at = Some(at(10, 0) + Duration::hours(end));
        f.winner_team_id = Some(winner);
        f.maps_won_a = a;
        f.maps_won_b = b;
    })
    .await;
    settle(app, m).await;
}

async fn ok(app: &AuthApp, uri: &str, token: &str) -> Value {
    let (status, body) = app.get(uri, token).await;
    assert_eq!(status, StatusCode::OK, "{uri}: {body}");
    body
}

async fn profile(app: &AuthApp, user: &str, query: &str, token: &str) -> Value {
    ok(app, &format!("/api/users/{user}/profile{query}"), token).await
}

async fn history(app: &AuthApp, user: &str, query: &str, token: &str) -> Value {
    ok(app, &format!("/api/users/{user}/picks{query}"), token).await
}

fn match_ids(body: &Value) -> Vec<i64> {
    body["picks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["match"]["id"].as_i64().unwrap())
        .collect()
}

/// Signs in Alice, Picker and the owner; returns Alice's token.
async fn sign_in_all(app: &AuthApp) -> String {
    app.sign_in(PICKER, GUILD).await;
    app.sign_in(OWNER, GUILD).await;
    app.sign_in(ALICE, GUILD).await
}

#[tokio::test]
async fn needs_a_session_valid_queries_and_a_member_of_the_guild() {
    let app = auth_app(None).await;
    for uri in [
        format!("/api/users/{ALICE}/profile"),
        format!("/api/users/{ALICE}/picks?state=open"),
    ] {
        let (status, body) = send(&app.app, "GET", &uri, &[], None).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{uri}");
        assert_eq!(body["error"], "unauthenticated");
    }

    let token = sign_in_all(&app).await;
    for query in [
        "/profile?event_id=abc",
        "/picks",
        "/picks?state=upcoming",
        "/picks?state=open&event_id=x",
        "/picks?state=settled&cursor=nope",
    ] {
        let (status, body) = app
            .get(&format!("/api/users/{PICKER}{query}"), &token)
            .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{query}: {body}");
        assert_eq!(body["error"], "invalid_query");
    }

    // Never signed in here, or unknown: not found.
    for user in [STRANGER, "123"] {
        for path in ["profile", "picks?state=open"] {
            let (status, body) = app.get(&format!("/api/users/{user}/{path}"), &token).await;
            assert_eq!(status, StatusCode::NOT_FOUND, "{user} {path}");
            assert_eq!(body["error"], "not_found");
        }
    }
    // Picker is a member of this guild only.
    let other = app.sign_in(ALICE, OTHER_GUILD).await;
    let (status, _) = app
        .get(&format!("/api/users/{PICKER}/profile"), &other)
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // A member without picks: no rank, nothing counted.
    let body = profile(&app, OWNER, "", &token).await;
    let nothing = json!({ "hits": 0, "decided": 0, "rate": null });
    assert_eq!(
        body,
        json!({
            "user_id": OWNER, "display_name": "owner",
            "avatar_url": format!("/api/img/avatar/{OWNER}?v=0"),
            "is_me": false, "event_id": null, "events": [],
            "rank": null, "previous_rank": null,
            "points": 0, "provisional_points": 0, "provisional_picks": 0, "settled_picks": 0,
            "accuracy": {
                "winner": nothing, "series": nothing,
                "map_winner": nothing, "map_score": nothing,
            },
            "picks": { "open": 0, "live": 0, "settled": 0, "total": 0 },
            "live": false,
        })
    );
    assert_eq!(profile(&app, ALICE, "", &token).await["is_me"], true);
}

/// AC 1: for every member and filter, the profile shows the leaderboard's
/// numbers.
#[tokio::test]
async fn profile_numbers_equal_the_leaderboard_for_the_same_filter() {
    let app = auth_app(None).await;
    let s = seed(&app).await;
    let token = sign_in_all(&app).await;
    let (a, b) = (s.a, s.b);

    // Event 1: two finished matches; event 2: one finished and one live.
    let m1 = bo3(&app, s.ev, &s, 10).await;
    let m2 = bo3(&app, s.ev, &s, 11).await;
    let m3 = bo3(&app, s.ev2, &s, 12).await;
    let live = bo3(&app, s.ev2, &s, 13).await;
    pick(&app, GUILD, ALICE, m1, full(&s, a, 2, 0, (13, 7))).await;
    pick(&app, GUILD, PICKER, m1, full(&s, a, 2, 1, (13, 5))).await;
    pick(&app, GUILD, OWNER, m1, winner_only(b)).await;
    pick(&app, GUILD, ALICE, m2, winner_only(b)).await;
    pick(&app, GUILD, PICKER, m2, full(&s, a, 2, 1, (7, 13))).await;
    pick(&app, GUILD, PICKER, m3, winner_only(a)).await;
    pick(&app, GUILD, OWNER, m3, full(&s, a, 2, 0, (13, 11))).await;
    pick(&app, GUILD, ALICE, live, full(&s, a, 2, 0, (13, 7))).await;
    pick(&app, GUILD, OWNER, live, full(&s, b, 0, 2, (7, 13))).await;

    map1(&app, m1, &s, (13, 7)).await;
    finish(&app, m1, a, 2, 0, 2).await;
    map1(&app, m2, &s, (13, 9)).await;
    finish(&app, m2, a, 2, 1, 3).await;
    map1(&app, m3, &s, (13, 11)).await;
    finish(&app, m3, a, 2, 0, 4).await;
    map1(&app, live, &s, (13, 7)).await;
    update(&app, live, |f| {
        f.status = MatchStatus::Live;
        f.started_at = Some(at(13, 0));
        f.maps_won_a = 1;
    })
    .await;
    settle(&app, live).await;

    for query in [
        String::new(),
        format!("?event_id={}", s.ev.0),
        format!("?event_id={}", s.ev2.0),
    ] {
        let board = ok(&app, &format!("/api/leaderboard{query}"), &token).await;
        let rows = board["rows"].as_array().unwrap();
        assert!(!rows.is_empty(), "{query}");
        for row in rows {
            let user = row["user_id"].as_str().unwrap();
            let p = profile(&app, user, &query, &token).await;
            let acc = &p["accuracy"];
            assert_eq!(p["event_id"], board["event_id"], "{query}");
            for key in [
                "rank",
                "previous_rank",
                "points",
                "provisional_points",
                "provisional_picks",
                "settled_picks",
            ] {
                assert_eq!(p[key], row[key], "{user} {key} {query}");
            }
            assert_eq!(acc["winner"]["hits"], row["winner_hits"], "{user} {query}");
            assert_eq!(acc["winner"]["decided"], row["winner_decided"]);
            assert_eq!(acc["winner"]["rate"], row["winner_accuracy"]);
            assert_eq!(acc["series"]["hits"], row["series_hits"]);
            assert_eq!(acc["map_score"]["hits"], row["map_hits"]);
        }
    }

    // Spot checks of the extra counters, all time.
    let alice = profile(&app, ALICE, "", &token).await;
    assert_eq!(alice["live"], true);
    assert_eq!(
        alice["picks"],
        json!({ "open": 0, "live": 1, "settled": 2, "total": 3 })
    );
    // m1: winner, series and map 1 exact; m2: winner miss; live: map 1 hit
    // so far, winner and series pending.
    assert_eq!(
        alice["accuracy"]["winner"],
        json!({ "hits": 1, "decided": 2, "rate": 0.5 })
    );
    assert_eq!(
        alice["accuracy"]["series"],
        json!({ "hits": 1, "decided": 1, "rate": 1.0 })
    );
    assert_eq!(
        alice["accuracy"]["map_winner"],
        json!({ "hits": 2, "decided": 2, "rate": 1.0 })
    );
    assert_eq!(
        alice["accuracy"]["map_score"],
        json!({ "hits": 2, "decided": 2, "rate": 1.0 })
    );
    let picker = profile(&app, PICKER, &format!("?event_id={}", s.ev.0), &token).await;
    assert_eq!(
        picker["accuracy"]["map_winner"],
        json!({ "hits": 1, "decided": 2, "rate": 0.5 })
    );
    assert_eq!(
        picker["picks"],
        json!({ "open": 0, "live": 0, "settled": 2, "total": 2 })
    );
}

/// AC 2: a void pick is listed under settled, marked void, and counts for
/// no accuracy.
#[tokio::test]
async fn void_picks_are_listed_and_left_out_of_accuracy() {
    let app = auth_app(None).await;
    let s = seed(&app).await;
    let token = sign_in_all(&app).await;
    let hit = bo3(&app, s.ev, &s, 10).await;
    let voided = bo3(&app, s.ev, &s, 11).await;
    pick(&app, GUILD, ALICE, hit, winner_only(s.a)).await;
    pick(&app, GUILD, ALICE, voided, winner_only(s.a)).await;
    finish(&app, hit, s.a, 2, 0, 1).await;
    // First a miss…
    finish(&app, voided, s.b, 0, 2, 2).await;
    let before = profile(&app, ALICE, "", &token).await;
    assert_eq!(before["accuracy"]["winner"]["decided"], 2);
    assert_eq!(before["settled_picks"], 2);

    // …then an admin voids the match.
    update(&app, voided, |f| f.voided = true).await;
    settle(&app, voided).await;
    let after = profile(&app, ALICE, "", &token).await;
    assert_eq!(
        after["accuracy"]["winner"],
        json!({ "hits": 1, "decided": 1, "rate": 1.0 })
    );
    assert_eq!(after["settled_picks"], 1);
    assert_eq!(after["points"], 3);
    // Still a settled pick in the history.
    assert_eq!(after["picks"]["settled"], 2);

    let list = history(&app, ALICE, "?state=settled", &token).await;
    assert_eq!(match_ids(&list), [voided.0, hit.0]);
    let void = &list["picks"][0];
    assert_eq!(void["match"]["voided"], true);
    assert_eq!(void["result"]["state"], "void");
    assert_eq!(void["result"]["points"], 0);
    assert_eq!(void["result"]["breakdown"]["winner"]["outcome"], "void");
    assert_eq!(void["pick"]["winner_team_id"], s.a.0);
    assert_eq!(list["picks"][1]["result"]["state"], "final");

    // A match voided before it started is settled too, not open.
    let early = bo3(&app, s.ev, &s, 20).await;
    pick(&app, GUILD, ALICE, early, winner_only(s.b)).await;
    update(&app, early, |f| f.voided = true).await;
    let list = history(&app, ALICE, "?state=settled", &token).await;
    assert!(match_ids(&list).contains(&early.0));
    assert_eq!(
        history(&app, ALICE, "?state=open", &token).await["picks"],
        json!([])
    );
}

#[tokio::test]
async fn lists_open_live_and_settled_picks_for_any_member() {
    let app = auth_app(None).await;
    let s = seed(&app).await;
    let token = sign_in_all(&app).await;
    let open = bo3(&app, s.ev, &s, 18).await;
    let later = bo3(&app, s.ev2, &s, 20).await;
    let live = bo3(&app, s.ev, &s, 10).await;
    let done = bo3(&app, s.ev2, &s, 8).await;
    for m in [open, later, live, done] {
        pick(&app, GUILD, PICKER, m, full(&s, s.a, 2, 0, (13, 7))).await;
    }
    pick(&app, GUILD, ALICE, open, winner_only(s.b)).await;
    finish(&app, done, s.a, 2, 0, 0).await;
    map1(&app, live, &s, (13, 7)).await;
    update(&app, live, |f| {
        f.status = MatchStatus::Live;
        f.started_at = Some(at(10, 0));
        f.maps_won_a = 1;
    })
    .await;
    settle(&app, live).await;

    // Alice looks at Picker's history: picks are public.
    let body = history(&app, PICKER, "?state=open", &token).await;
    assert_eq!(body["state"], "open");
    assert_eq!(match_ids(&body), [open.0, later.0]);
    assert_eq!(body["next_cursor"], Value::Null);
    let first = &body["picks"][0];
    assert_eq!(first["match"]["pickable"], true);
    assert_eq!(first["match"]["locked"], false);
    assert_eq!(first["match"]["team_a"]["name"], "Vitality");
    assert_eq!(first["match"]["event"]["name"], "IEM Test");
    // The guild's pick count, and Picker's own parts.
    assert_eq!(first["match"]["pick_count"], 2);
    assert_eq!(first["pick"]["series"], json!({ "a": 2, "b": 0 }));
    assert_eq!(
        first["pick"]["maps"][0]["rounds"],
        json!({ "a": 13, "b": 7 })
    );
    assert_eq!(first["result"], Value::Null);
    let filtered = history(
        &app,
        PICKER,
        &format!("?state=open&event_id={}", s.ev2.0),
        &token,
    )
    .await;
    assert_eq!(match_ids(&filtered), [later.0]);

    let body = history(&app, PICKER, "?state=live", &token).await;
    assert_eq!(match_ids(&body), [live.0]);
    let r = &body["picks"][0]["result"];
    assert_eq!(r["state"], "provisional");
    // Map winner 1 and map score 2 so far.
    assert_eq!(r["points"], 3);
    assert_eq!(profile(&app, PICKER, "", &token).await["live"], true);

    let body = history(&app, PICKER, "?state=settled", &token).await;
    assert_eq!(match_ids(&body), [done.0]);
    assert_eq!(body["picks"][0]["result"]["state"], "final");
    assert_eq!(
        body["picks"][0]["result"]["breakdown"]["series"]["outcome"],
        "hit"
    );

    // At 18:00 the open match locks: still open, no longer pickable.
    app.clock.set(at(18, 0));
    let body = history(&app, PICKER, "?state=open", &token).await;
    assert_eq!(body["picks"][0]["match"]["pickable"], false);
    assert_eq!(body["picks"][0]["match"]["locked"], true);
}

#[tokio::test]
async fn settled_picks_come_in_pages() {
    let app = auth_app(None).await;
    let s = seed(&app).await;
    let token = sign_in_all(&app).await;
    let n = usize::try_from(SETTLED_PAGE).unwrap() + 1;
    let mut ids = Vec::new();
    for i in 0..n {
        let m = bo3(&app, s.ev, &s, 8).await;
        pick(&app, GUILD, ALICE, m, winner_only(s.a)).await;
        update(&app, m, |f| {
            f.status = MatchStatus::Finished;
            f.ended_at = Some(at(10, 0) + Duration::minutes(i64::try_from(i).unwrap()));
        })
        .await;
        ids.push(m.0);
    }
    // Latest end first.
    ids.reverse();

    let first = history(&app, ALICE, "?state=settled", &token).await;
    assert_eq!(match_ids(&first), ids[..n - 1]);
    let cursor = first["next_cursor"].as_str().unwrap();
    let second = history(
        &app,
        ALICE,
        &format!("?state=settled&cursor={cursor}"),
        &token,
    )
    .await;
    assert_eq!(match_ids(&second), ids[n - 1..]);
    assert_eq!(second["next_cursor"], Value::Null);
}
