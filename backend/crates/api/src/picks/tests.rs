//! Pick API over HTTP (T10), on the fake Discord and a fake clock at
//! 09:00; the seeded Bo3 starts (and locks) at 18:00.

use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use chrono::{DateTime, Duration, TimeZone, Utc};
use domain::{Clock, EventId, EventStatus, EventTier, MatchFields, MatchFormat, NewEvent, NewTeam};
use serde_json::{Value, json};
use store::guilds::FollowedEvent;
use tower::ServiceExt;

use crate::test_util::{ALICE, AuthApp, GUILD, OTHER_GUILD, OWNER, PICKER, auth_app, send};

fn at(h: u32, m: u32, s: u32) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 11, 20, h, m, s).unwrap()
}

struct Seed {
    a: i64,
    b: i64,
    /// Followed in [`GUILD`] and [`OTHER_GUILD`], Bo3 at 18:00.
    bo3: i64,
    /// Followed, Bo1 at 18:00.
    bo1: i64,
    /// Followed, team B TBD.
    tbd: i64,
    /// In an event nobody follows.
    unfollowed: i64,
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
    let followed = event(app, "IEM Test").await;
    let other = event(app, "Unfollowed Cup").await;
    for guild in [GUILD, OTHER_GUILD] {
        store::guilds::follow_event(
            &app.pool,
            &FollowedEvent {
                guild_id: guild.into(),
                event_id: followed,
                followed_by: OWNER.into(),
                followed_at: at(8, 0, 0),
            },
        )
        .await
        .unwrap();
    }
    let insert = |ev, a, b, f| {
        let pool = app.pool.clone();
        async move {
            store::matches::insert(
                &pool,
                &MatchFields::scheduled(ev, a, b, f, Some(at(18, 0, 0))),
            )
            .await
            .unwrap()
            .0
        }
    };
    Seed {
        a: a.0,
        b: b.0,
        bo3: insert(followed, Some(a), Some(b), MatchFormat::Bo3).await,
        bo1: insert(followed, Some(a), Some(b), MatchFormat::Bo1).await,
        tbd: insert(followed, Some(a), None, MatchFormat::Bo3).await,
        unfollowed: insert(other, Some(a), Some(b), MatchFormat::Bo3).await,
    }
}

async fn setup(guild: Option<&str>) -> (AuthApp, Seed) {
    let app = auth_app(guild).await;
    let s = seed(&app).await;
    (app, s)
}

async fn call(
    app: &AuthApp,
    method: &str,
    uri: &str,
    token: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let auth = format!("Bearer {token}");
    send(
        &app.app,
        method,
        uri,
        &[("authorization", auth.as_str())],
        body,
    )
    .await
}

async fn put(app: &AuthApp, m: i64, token: &str, body: Value) -> (StatusCode, Value) {
    call(
        app,
        "PUT",
        &format!("/api/matches/{m}/pick"),
        token,
        Some(body),
    )
    .await
}

async fn picks(app: &AuthApp, m: i64, token: &str) -> Value {
    let (status, body) = app.get(&format!("/api/matches/{m}/picks"), token).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body
}

#[tokio::test]
async fn winner_only_map_only_and_full_picks_are_accepted() {
    let (app, s) = setup(Some(GUILD)).await;
    let alice = app.sign_in(ALICE, GUILD).await;

    let (status, body) = put(&app, s.bo1, &alice, json!({ "winner_team_id": s.a })).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["match_id"], s.bo1);
    assert_eq!(body["winner_team_id"], s.a);
    assert_eq!(body["series"], Value::Null);
    assert_eq!(body["maps"], json!([]));
    assert_eq!(body["edited"], false);

    let map_only =
        json!({ "maps": [{ "map_number": 2, "winner_team_id": s.b, "overtime": true }] });
    let (status, body) = put(&app, s.tbd, &alice, map_only.clone()).await;
    assert_eq!(status, StatusCode::CONFLICT, "TBD: {body}");
    let (status, body) = put(&app, s.bo3, &alice, map_only).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["winner_team_id"], Value::Null);
    assert_eq!(
        body["maps"],
        json!([{ "map_number": 2, "winner_team_id": s.b, "rounds": null, "overtime": true }])
    );

    // Full pick; the series score sets the winner, maps come back sorted.
    let full = json!({
        "series": { "a": 2, "b": 1 },
        "maps": [
            { "map_number": 3, "winner_team_id": s.a, "rounds": { "a": 13, "b": 11 } },
            { "map_number": 1, "winner_team_id": s.a, "rounds": { "a": 13, "b": 4 } },
            { "map_number": 2, "winner_team_id": s.b },
        ],
    });
    let (status, body) = put(&app, s.bo3, &alice, full).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["winner_team_id"], s.a);
    assert_eq!(body["series"], json!({ "a": 2, "b": 1 }));
    // Saved twice at the same instant: still an edit.
    assert_eq!(body["edited"], true);
    let numbers: Vec<i64> = body["maps"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["map_number"].as_i64().unwrap())
        .collect();
    assert_eq!(numbers, [1, 2, 3]);
    assert_eq!(body["maps"][2]["rounds"], json!({ "a": 13, "b": 11 }));
}

#[tokio::test]
async fn invalid_picks_and_bodies_are_rejected() {
    let (app, s) = setup(Some(GUILD)).await;
    let alice = app.sign_in(ALICE, GUILD).await;
    for (m, body) in [
        (s.bo1, json!({ "series": { "a": 1, "b": 0 } })),
        (s.bo3, json!({ "series": { "a": 2, "b": 2 } })),
        (
            s.bo3,
            json!({ "winner_team_id": s.b, "series": { "a": 2, "b": 0 } }),
        ),
        (s.bo3, json!({ "winner_team_id": 999 })),
        (
            s.bo3,
            json!({ "maps": [{ "map_number": 4, "winner_team_id": s.a }] }),
        ),
        (
            s.bo1,
            json!({ "maps": [{ "map_number": 2, "winner_team_id": s.a }] }),
        ),
        (
            s.bo3,
            json!({ "maps": [{ "map_number": 1, "winner_team_id": s.a, "rounds": { "a": 13, "b": 12 } }] }),
        ),
        (
            s.bo3,
            json!({ "maps": [{ "map_number": 1, "winner_team_id": s.a, "rounds": { "a": 13, "b": 3 }, "overtime": true }] }),
        ),
    ] {
        let (status, res) = put(&app, m, &alice, body.clone()).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body} → {res}");
        assert_eq!(res["error"], "invalid_pick", "{body}");
        assert!(res["message"].as_str().is_some_and(|m| !m.is_empty()));
    }
    // Not the pick's shape at all.
    for body in [
        json!({ "winner": "a" }),
        json!({ "maps": [{ "map_number": 1 }] }),
        json!([1]),
    ] {
        let (status, res) = put(&app, s.bo3, &alice, body.clone()).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body} → {res}");
        assert_eq!(res["error"], "invalid_body", "{body}");
    }
    assert!(
        store::picks::list_for_match(&app.pool, GUILD, domain::MatchId(s.bo3))
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn picks_on_tbd_unfollowed_and_unknown_matches_are_rejected() {
    let (app, s) = setup(Some(GUILD)).await;
    let alice = app.sign_in(ALICE, GUILD).await;
    let winner = json!({ "winner_team_id": s.a });
    for m in [s.tbd, s.unfollowed] {
        let (status, body) = put(&app, m, &alice, winner.clone()).await;
        assert_eq!(status, StatusCode::CONFLICT, "{body}");
        assert_eq!(body["error"], "not_pickable");
        assert_eq!(picks(&app, m, &alice).await["pickable"], false);
    }
    let (status, body) = put(&app, 9999, &alice, winner).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    let (status, _) = app.get("/api/matches/9999/picks", &alice).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn a_request_one_second_after_lock_at_is_rejected_before_the_ticker_ran() {
    let (app, s) = setup(Some(GUILD)).await;
    let alice = app.sign_in(ALICE, GUILD).await;
    let (status, _) = put(&app, s.bo3, &alice, json!({ "winner_team_id": s.a })).await;
    assert_eq!(status, StatusCode::OK);
    let view = picks(&app, s.bo3, &alice).await;
    assert_eq!(
        (view["pickable"].clone(), view["locked"].clone()),
        (json!(true), json!(false))
    );

    app.clock.set(at(18, 0, 1));
    let m = store::matches::get(&app.pool, domain::MatchId(s.bo3))
        .await
        .unwrap()
        .unwrap();
    assert!(!m.fields.locked, "the ticker has not run");

    let (status, body) = put(&app, s.bo3, &alice, json!({ "winner_team_id": s.b })).await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["error"], "pick_locked");
    let (status, body) = call(
        &app,
        "DELETE",
        &format!("/api/matches/{}/pick", s.bo3),
        &alice,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["error"], "pick_locked");
    // An empty PUT is a delete, and locked too.
    let (status, body) = put(&app, s.bo3, &alice, json!({})).await;
    assert_eq!(body["error"], "pick_locked", "{status}");

    // The pick is unchanged and the view says it is read-only.
    let view = picks(&app, s.bo3, &alice).await;
    assert_eq!(
        (view["pickable"].clone(), view["locked"].clone()),
        (json!(false), json!(true))
    );
    assert_eq!(view["picks"][0]["winner_team_id"], s.a);
}

#[tokio::test]
async fn a_match_locked_by_the_ticker_is_read_only() {
    let (app, s) = setup(Some(GUILD)).await;
    let alice = app.sign_in(ALICE, GUILD).await;
    // E.g. the match went live early: the ticker locks it before lock_at.
    sqlx::query("UPDATE matches SET status = 'live' WHERE id = ?")
        .bind(s.bo3)
        .execute(&app.pool)
        .await
        .unwrap();
    sync::lock::lock_due(&app.pool, app.clock.now())
        .await
        .unwrap();
    let (status, body) = put(&app, s.bo3, &alice, json!({ "winner_team_id": s.a })).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(body["error"], "pick_locked");
}

#[tokio::test]
async fn a_pick_can_be_edited_until_the_lock_and_every_save_is_a_revision() {
    let (app, s) = setup(Some(GUILD)).await;
    let alice = app.sign_in(ALICE, GUILD).await;
    let picker = app.sign_in(PICKER, GUILD).await;
    let owner = app.sign_in(OWNER, GUILD).await;

    let bodies = [
        json!({ "winner_team_id": s.a }),
        json!({ "winner_team_id": s.a, "series": { "a": 2, "b": 0 } }),
        json!({ "maps": [{ "map_number": 1, "winner_team_id": s.b }] }),
    ];
    for (i, b) in bodies.iter().enumerate() {
        let (status, body) = put(&app, s.bo3, &alice, b.clone()).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["edited"], i > 0);
        app.clock.advance(Duration::minutes(10));
    }

    // Everyone sees the current pick with the edited marker.
    let view = picks(&app, s.bo3, &picker).await;
    let p = &view["picks"][0];
    assert_eq!(p["user_id"], ALICE);
    assert_eq!(p["edited"], true);
    assert_eq!(p["created_at"], "2026-11-20T09:00:00Z");
    assert_eq!(p["updated_at"], "2026-11-20T09:20:00Z");
    assert_eq!(p["winner_team_id"], Value::Null, "the part was removed");
    assert_eq!(p["series"], Value::Null);

    // Admins see the full history; members don't.
    let uri = format!("/api/admin/matches/{}/pick-revisions", s.bo3);
    let (status, body) = call(&app, "GET", &uri, &picker, None).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    let (status, body) = call(&app, "GET", &uri, &owner, None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["match_id"], s.bo3);
    let history = &body["picks"][0];
    assert_eq!(history["user_id"], ALICE);
    assert_eq!(history["display_name"], "ali");
    let revs = history["revisions"].as_array().unwrap();
    assert_eq!(revs.len(), 3);
    assert_eq!(revs[0]["edited_at"], "2026-11-20T09:00:00Z");
    assert_eq!(revs[1]["snapshot"]["series"], json!({ "a": 2, "b": 0 }));
    assert_eq!(revs[2]["snapshot"]["maps"][0]["winner_team_id"], s.b);
    let (status, _) = call(
        &app,
        "GET",
        "/api/admin/matches/9999/pick-revisions",
        &owner,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn removing_every_part_or_deleting_removes_the_pick() {
    let (app, s) = setup(Some(GUILD)).await;
    let alice = app.sign_in(ALICE, GUILD).await;
    let uri = format!("/api/matches/{}/pick", s.bo3);

    put(&app, s.bo3, &alice, json!({ "winner_team_id": s.a })).await;
    let (status, _) = put(
        &app,
        s.bo3,
        &alice,
        json!({ "winner_team_id": null, "maps": [] }),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert_eq!(picks(&app, s.bo3, &alice).await["picks"], json!([]));

    put(&app, s.bo3, &alice, json!({ "winner_team_id": s.a })).await;
    let (status, _) = call(&app, "DELETE", &uri, &alice, None).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert_eq!(picks(&app, s.bo3, &alice).await["picks"], json!([]));
    // Idempotent.
    let (status, _) = call(&app, "DELETE", &uri, &alice, None).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    // Not pickable → no delete either.
    let (status, body) = call(
        &app,
        "DELETE",
        &format!("/api/matches/{}/pick", s.unfollowed),
        &alice,
        None,
    )
    .await;
    assert_eq!(body["error"], "not_pickable", "{status}");
}

#[tokio::test]
async fn everyone_sees_all_picks_with_names_and_aggregates() {
    let (app, s) = setup(Some(GUILD)).await;
    let alice = app.sign_in(ALICE, GUILD).await;
    let picker = app.sign_in(PICKER, GUILD).await;
    let owner = app.sign_in(OWNER, GUILD).await;

    put(&app, s.bo3, &alice, json!({ "series": { "a": 2, "b": 1 }, "maps": [{ "map_number": 1, "winner_team_id": s.a }] })).await;
    put(
        &app,
        s.bo3,
        &picker,
        json!({ "winner_team_id": s.b, "maps": [{ "map_number": 1, "winner_team_id": s.b }] }),
    )
    .await;
    put(&app, s.bo3, &owner, json!({ "winner_team_id": s.a })).await;

    // Visible to a member without a pick, before the lock.
    let view = picks(&app, s.bo3, &owner).await;
    assert_eq!(view["match_id"], s.bo3);
    assert_eq!(view["lock_at"], "2026-11-20T18:00:00Z");
    let names: Vec<(&str, &str)> = view["picks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| {
            (
                p["user_id"].as_str().unwrap(),
                p["display_name"].as_str().unwrap(),
            )
        })
        .collect();
    assert_eq!(
        names,
        [(ALICE, "ali"), (PICKER, "picker"), (OWNER, "owner")]
    );
    assert_eq!(
        view["picks"][0]["avatar_url"],
        format!("/api/img/avatar/{ALICE}?v=abc123")
    );
    assert_eq!(
        view["picks"][0]["winner_team_id"], s.a,
        "derived from the series"
    );

    let st = &view["stats"];
    assert_eq!(st["total"], 3);
    assert_eq!(
        st["winner"],
        json!({ "a": 2, "b": 1, "pct_a": 67, "pct_b": 33 })
    );
    assert_eq!(
        st["series"],
        json!([
            { "a": 2, "b": 0, "count": 0, "pct": 0 },
            { "a": 2, "b": 1, "count": 1, "pct": 100 },
            { "a": 1, "b": 2, "count": 0, "pct": 0 },
            { "a": 0, "b": 2, "count": 0, "pct": 0 },
        ])
    );
    assert_eq!(
        st["maps"][0],
        json!({ "map_number": 1, "a": 1, "b": 1, "pct_a": 50, "pct_b": 50 })
    );
    assert_eq!(st["maps"][2]["a"], 0);

    // Still visible after the lock.
    app.clock.set(at(19, 0, 0));
    assert_eq!(
        picks(&app, s.bo3, &alice).await["picks"]
            .as_array()
            .unwrap()
            .len(),
        3
    );
}

#[tokio::test]
async fn another_guilds_picks_are_not_visible() {
    let (app, s) = setup(None).await;
    let here = app.sign_in(OWNER, GUILD).await;
    let there = app.sign_in(ALICE, OTHER_GUILD).await;
    let (status, _) = put(&app, s.bo3, &there, json!({ "winner_team_id": s.b })).await;
    assert_eq!(status, StatusCode::OK);
    let view = picks(&app, s.bo3, &here).await;
    assert_eq!(view["picks"], json!([]));
    assert_eq!(view["stats"]["total"], 0);
    let (_, mine) = app.get("/api/me/picks", &here).await;
    assert_eq!(mine["picks"], json!([]));
}

#[tokio::test]
async fn an_edit_changes_the_etag_so_the_next_poll_sees_it() {
    let (app, s) = setup(Some(GUILD)).await;
    let alice = app.sign_in(ALICE, GUILD).await;
    let picker = app.sign_in(PICKER, GUILD).await;
    let uri = format!("/api/matches/{}/picks", s.bo3);
    let poll = |etag: Option<String>| {
        let mut req = Request::get(&uri).header(header::AUTHORIZATION, format!("Bearer {picker}"));
        if let Some(tag) = etag {
            req = req.header(header::IF_NONE_MATCH, tag);
        }
        let app = app.app.clone();
        async move {
            let res = app.oneshot(req.body(Body::empty()).unwrap()).await.unwrap();
            let tag = res
                .headers()
                .get(header::ETAG)
                .map(|v| v.to_str().unwrap().to_owned());
            (res.status(), tag)
        }
    };

    let (status, tag) = poll(None).await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = poll(tag.clone()).await;
    assert_eq!(status, StatusCode::NOT_MODIFIED);

    put(&app, s.bo3, &alice, json!({ "winner_team_id": s.a })).await;
    let (status, new_tag) = poll(tag).await;
    assert_eq!(status, StatusCode::OK, "the edit shows on the next poll");
    assert_ne!(new_tag, None);
}

#[tokio::test]
async fn my_picks_lists_the_callers_picks() {
    let (app, s) = setup(Some(GUILD)).await;
    let alice = app.sign_in(ALICE, GUILD).await;
    let picker = app.sign_in(PICKER, GUILD).await;
    put(&app, s.bo1, &alice, json!({ "winner_team_id": s.b })).await;
    put(&app, s.bo3, &alice, json!({ "series": { "a": 0, "b": 2 } })).await;
    put(&app, s.bo3, &picker, json!({ "winner_team_id": s.a })).await;

    let (status, body) = app.get("/api/me/picks", &alice).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let mine = body["picks"].as_array().unwrap();
    assert_eq!(mine.len(), 2);
    let mut ids: Vec<i64> = mine
        .iter()
        .map(|p| p["match_id"].as_i64().unwrap())
        .collect();
    ids.sort_unstable();
    assert_eq!(ids, [s.bo3, s.bo1]);
    let bo3 = mine.iter().find(|p| p["match_id"] == s.bo3).unwrap();
    assert_eq!(bo3["winner_team_id"], s.b);
    assert_eq!(bo3["series"], json!({ "a": 0, "b": 2 }));
}

#[tokio::test]
async fn every_pick_route_needs_a_session() {
    let (app, s) = setup(Some(GUILD)).await;
    for (method, uri, body) in [
        (
            "PUT",
            format!("/api/matches/{}/pick", s.bo3),
            Some(json!({ "winner_team_id": s.a })),
        ),
        ("DELETE", format!("/api/matches/{}/pick", s.bo3), None),
        ("GET", format!("/api/matches/{}/picks", s.bo3), None),
        ("GET", "/api/me/picks".to_owned(), None),
    ] {
        let (status, res) = send(&app.app, method, &uri, &[], body).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{method} {uri}");
        assert_eq!(res["error"], "unauthenticated");
    }
}
