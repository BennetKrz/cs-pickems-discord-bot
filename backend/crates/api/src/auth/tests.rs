//! Sign-in, sessions and the admin check against a fake Discord.

use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use chrono::Duration;
use serde_json::{Value, json};
use store::guilds::GuildSettings;
use tower::ServiceExt;

use super::*;
use crate::test_util::*;

async fn post_token(a: &AuthApp, code: &str, guild: Option<&str>) -> (StatusCode, Value) {
    let body = match guild {
        Some(g) => json!({ "code": code, "guild_id": g }),
        None => json!({ "code": code }),
    };
    send(&a.app, "POST", "/api/auth/token", &[], Some(body)).await
}

async fn session_count(pool: &SqlitePool) -> i64 {
    sqlx::query_scalar("SELECT COUNT(*) FROM sessions")
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn set_admin_role(pool: &SqlitePool, role: Option<&str>) {
    store::guilds::upsert_settings(
        pool,
        &GuildSettings {
            admin_role_id: role.map(str::to_owned),
            ..GuildSettings::new(GUILD, t0())
        },
    )
    .await
    .unwrap();
}

#[tokio::test]
async fn config_names_the_client_id() {
    let a = auth_app(None).await;
    let (status, body) = send(&a.app, "GET", "/api/auth/config", &[], None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, json!({ "client_id": "424242424242424242" }));
}

#[tokio::test]
async fn sign_in_creates_a_session_and_a_player() {
    let a = auth_app(Some(GUILD)).await;
    let (status, body) = post_token(&a, &code(ALICE), Some(GUILD)).await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let token = body["session_token"].as_str().unwrap().to_owned();
    assert!(is_token(&token), "{token}");
    assert_eq!(body["access_token"], format!("at-{ALICE}"));
    let expires: DateTime<Utc> = body["expires_at"].as_str().unwrap().parse().unwrap();
    assert_eq!(expires, t0() + Duration::days(7));
    assert_eq!(
        body["me"],
        json!({
            "user_id": ALICE,
            "guild_id": GUILD,
            "username": "alice",
            "display_name": "ali",
            "avatar_url": format!("/api/img/avatar/{ALICE}?v=abc123"),
            "is_admin": false,
        })
    );

    // The user and the membership are upserted: that makes a player.
    let user = store::users::get_user(&a.pool, ALICE)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        (
            user.username.as_str(),
            user.global_name.as_deref(),
            user.avatar_hash.as_deref()
        ),
        ("alice", Some("Alice"), Some("abc123"))
    );
    let member = store::users::get_member(&a.pool, GUILD, ALICE)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(member.nick.as_deref(), Some("ali"));
    assert_eq!(member.last_seen_at, t0());

    // Only the token's hash is stored.
    let stored: Vec<String> = sqlx::query_scalar("SELECT token_hash FROM sessions")
        .fetch_all(&a.pool)
        .await
        .unwrap();
    assert_eq!(stored, [hash_token(&token)]);
    assert_ne!(stored[0], token);

    // The session works.
    let (status, me) = a.get("/api/me", &token).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(me, body["me"]);
}

#[tokio::test]
async fn sign_in_answers_are_never_cached() {
    let a = auth_app(None).await;
    let req = Request::post("/api/auth/token")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(
            json!({ "code": code(ALICE), "guild_id": GUILD }).to_string(),
        ))
        .unwrap();
    let res = a.app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(res.headers()[header::CACHE_CONTROL], "no-store");
}

#[tokio::test]
async fn forged_or_missing_guild_claims_are_rejected_before_discord() {
    let a = auth_app(Some(GUILD)).await;
    // Alice really is a member of OTHER_GUILD, but this app serves GUILD.
    for (guild, status, error) in [
        (Some(OTHER_GUILD), StatusCode::FORBIDDEN, "wrong_guild"),
        (Some(FOREIGN_GUILD), StatusCode::FORBIDDEN, "wrong_guild"),
        (None, StatusCode::BAD_REQUEST, "guild_required"),
        (Some(""), StatusCode::BAD_REQUEST, "guild_required"),
        (Some("1 OR 1=1"), StatusCode::BAD_REQUEST, "invalid_request"),
    ] {
        let (got, body) = post_token(&a, &code(ALICE), guild).await;
        assert_eq!(
            (got, body["error"].as_str()),
            (status, Some(error)),
            "{guild:?}"
        );
    }
    assert_eq!(
        a.discord.calls(),
        Vec::<&str>::new(),
        "Discord is not asked"
    );
    assert_eq!(session_count(&a.pool).await, 0);
}

#[tokio::test]
async fn non_members_get_403_and_no_session() {
    let a = auth_app(None).await;
    // A Discord user who is not in the guild.
    let (status, body) = post_token(&a, &code(STRANGER), Some(GUILD)).await;
    assert_eq!(
        (status, body["error"].as_str()),
        (StatusCode::FORBIDDEN, Some("not_a_member"))
    );
    // A guild the bot is not in can't be verified.
    let (status, body) = post_token(&a, &code(ALICE), Some(FOREIGN_GUILD)).await;
    assert_eq!(
        (status, body["error"].as_str()),
        (StatusCode::FORBIDDEN, Some("not_a_member"))
    );

    assert_eq!(session_count(&a.pool).await, 0);
    assert_eq!(
        store::users::get_user(&a.pool, STRANGER).await.unwrap(),
        None
    );
}

#[tokio::test]
async fn bad_codes_get_401_and_outages_503() {
    let a = auth_app(Some(GUILD)).await;
    let (status, body) = post_token(&a, "used-or-forged", Some(GUILD)).await;
    assert_eq!(
        (status, body["error"].as_str()),
        (StatusCode::UNAUTHORIZED, Some("invalid_code"))
    );
    let (status, body) = post_token(&a, " ", Some(GUILD)).await;
    assert_eq!(
        (status, body["error"].as_str()),
        (StatusCode::BAD_REQUEST, Some("invalid_request"))
    );
    a.discord.set_down(true);
    let (status, body) = post_token(&a, &code(ALICE), Some(GUILD)).await;
    assert_eq!(
        (status, body["error"].as_str()),
        (StatusCode::SERVICE_UNAVAILABLE, Some("discord_unavailable"))
    );
    assert_eq!(session_count(&a.pool).await, 0);
}

#[tokio::test]
async fn sessions_expire_after_seven_days() {
    let a = auth_app(Some(GUILD)).await;
    let token = a.sign_in(ALICE, GUILD).await;

    a.clock.advance(Duration::days(7) - Duration::seconds(1));
    let (status, _) = a.get("/api/me", &token).await;
    assert_eq!(status, StatusCode::OK, "valid until the last second");

    a.clock.advance(Duration::seconds(1));
    let (status, body) = a.get("/api/me", &token).await;
    assert_eq!(
        (status, body["error"].as_str()),
        (StatusCode::UNAUTHORIZED, Some("unauthenticated"))
    );

    // The next sign-in purges the expired row.
    a.sign_in(MOD, GUILD).await;
    assert_eq!(session_count(&a.pool).await, 1);
}

#[tokio::test]
async fn missing_or_malformed_tokens_get_401() {
    let a = auth_app(Some(GUILD)).await;
    let token = a.sign_in(ALICE, GUILD).await;
    let unknown = "f".repeat(64);
    let cases = [
        String::new(),
        "Bearer".to_owned(),
        "Bearer xyz".to_owned(),
        format!("Basic {token}"),
        format!("Bearer {unknown}"),
        format!("Bearer {}", token.to_uppercase()),
    ];
    for auth in &cases {
        let headers: &[(&str, &str)] = if auth.is_empty() {
            &[]
        } else {
            &[("authorization", auth.as_str())]
        };
        let (status, body) = send(&a.app, "GET", "/api/me", headers, None).await;
        assert_eq!(
            (status, body["error"].as_str()),
            (StatusCode::UNAUTHORIZED, Some("unauthenticated")),
            "{auth:?}"
        );
    }
    // The scheme is case-insensitive.
    let lower = format!("bearer {token}");
    let (status, _) = send(&a.app, "GET", "/api/me", &[("authorization", &lower)], None).await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn pinning_the_guild_ends_sessions_of_other_guilds() {
    // Without DISCORD_GUILD_ID, any guild where the bot can verify the
    // membership is accepted.
    let a = auth_app(None).await;
    let token = a.sign_in(ALICE, OTHER_GUILD).await;
    let (status, me) = a.get("/api/me", &token).await;
    assert_eq!(
        (status, me["guild_id"].as_str()),
        (StatusCode::OK, Some(OTHER_GUILD))
    );

    // Restart with DISCORD_GUILD_ID = GUILD: that session no longer works.
    let AuthApp {
        pool, dir, discord, ..
    } = a;
    let b = auth_app_on(pool, dir, discord, Some(GUILD));
    let (status, _) = b.get("/api/me", &token).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn admins_are_the_owner_manage_server_and_the_configured_role() {
    let a = auth_app(Some(GUILD)).await;
    set_admin_role(&a.pool, Some(PICKEM_ROLE)).await;

    for (user, admin) in [(OWNER, true), (MOD, true), (PICKER, true), (ALICE, false)] {
        let token = a.sign_in(user, GUILD).await;
        let (_, me) = a.get("/api/me", &token).await;
        assert_eq!(me["is_admin"], admin, "{user}");

        // Admins pass the guard and reach the handler, which has no sync
        // worker in tests; members are stopped by the guard.
        let (status, body) = a.get("/api/admin/status", &token).await;
        let expected = if admin {
            (StatusCode::SERVICE_UNAVAILABLE, "sync_unavailable")
        } else {
            (StatusCode::FORBIDDEN, "forbidden")
        };
        assert_eq!(
            (status, body["error"].as_str().unwrap()),
            expected,
            "{user}"
        );
    }

    let (status, _) = send(&a.app, "GET", "/api/admin/status", &[], None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn admin_data_is_cached_for_five_minutes_but_the_role_setting_is_not() {
    let a = auth_app(Some(GUILD)).await;
    a.sign_in(ALICE, GUILD).await;
    // Sign-in asked for the membership and, for `me`, the guild.
    assert_eq!(a.discord.calls(), ["exchange", "me", "member", "guild"]);
    a.discord.clear_calls();

    assert!(!a.auth.is_admin(GUILD, ALICE).await.unwrap());
    assert_eq!(
        a.discord.calls(),
        Vec::<&str>::new(),
        "served from the cache"
    );

    // Alice becomes a moderator in Discord; the cache still says no.
    a.discord.set_roles(GUILD, ALICE, &[MOD_ROLE]);
    a.clock.advance(Duration::minutes(5) - Duration::seconds(1));
    assert!(!a.auth.is_admin(GUILD, ALICE).await.unwrap());

    // The configured role is read fresh: Alice's cached role counts at once.
    set_admin_role(&a.pool, Some(PLAIN_ROLE)).await;
    assert!(a.auth.is_admin(GUILD, ALICE).await.unwrap());
    set_admin_role(&a.pool, None).await;
    assert!(!a.auth.is_admin(GUILD, ALICE).await.unwrap());
    assert_eq!(a.discord.calls(), Vec::<&str>::new());

    // After 5 minutes Discord is asked again.
    a.clock.advance(Duration::seconds(1));
    assert!(a.auth.is_admin(GUILD, ALICE).await.unwrap());
    assert_eq!(a.discord.calls(), ["guild", "member"]);
}

#[tokio::test]
async fn a_discord_outage_blocks_admin_routes_but_not_me() {
    let a = auth_app(Some(GUILD)).await;
    let token = a.sign_in(OWNER, GUILD).await;
    a.clock.advance(Duration::minutes(6));
    a.discord.set_down(true);

    let (status, body) = a.get("/api/admin/status", &token).await;
    assert_eq!(
        (status, body["error"].as_str()),
        (StatusCode::SERVICE_UNAVAILABLE, Some("discord_unavailable"))
    );
    let (status, me) = a.get("/api/me", &token).await;
    assert_eq!(
        (status, me["is_admin"].as_bool()),
        (StatusCode::OK, Some(false))
    );
    assert_eq!(
        a.discord.calls().iter().filter(|c| **c == "guild").count(),
        2,
        "sign-in plus the first failed check; then Discord is left alone"
    );

    // Back up, but within the backoff: still unknown, Discord not asked.
    a.discord.set_down(false);
    a.discord.clear_calls();
    a.clock.advance(Duration::seconds(29));
    let (status, _) = a.get("/api/admin/status", &token).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(a.discord.calls(), Vec::<&str>::new());

    a.clock.advance(Duration::seconds(1));
    let (_, me) = a.get("/api/me", &token).await;
    assert_eq!(me["is_admin"], true);
}

#[test]
fn token_and_id_formats() {
    let mut h = HeaderMap::new();
    assert_eq!(bearer(&h), None);
    for (value, token) in [
        ("Bearer abc", Some("abc")),
        ("bearer  abc ", Some("abc")),
        ("BEARER abc", Some("abc")),
        ("Bearer ", None),
        ("Basic abc", None),
        ("abc", None),
    ] {
        h.insert(header::AUTHORIZATION, HeaderValue::from_static(value));
        assert_eq!(bearer(&h), token, "{value:?}");
    }

    assert!(is_token(&"0a".repeat(32)));
    assert!(!is_token(&"0A".repeat(32)));
    assert!(!is_token(&"0a".repeat(31)));

    assert!(is_snowflake("100000000000000001"));
    assert!(!is_snowflake(""));
    assert!(!is_snowflake("12a"));
    assert!(!is_snowflake(&"1".repeat(21)));

    let a = new_token().unwrap();
    let b = new_token().unwrap();
    assert!(is_token(&a) && a != b);
    assert_eq!(hash_token(&a).len(), 64);
}
