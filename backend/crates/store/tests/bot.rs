//! Bot repositories (T08): post claims, rate-limit counts, announced
//! results, pending views and the guild settings the bot changes.

mod common;

use chrono::Duration;
use common::{ALICE, BOB, GUILD, at, exec, seed, test_db};
use domain::{MatchId, PickParts};
use store::bot::{self, NewPost, PostKind};
use store::guilds::{self, FollowedEvent, GuildSettings};
use store::pending_views;

const CHANNEL: &str = "300000000000000001";

fn post<'a>(kind: PostKind, key: &'a str, minute: u32) -> NewPost<'a> {
    NewPost {
        guild_id: GUILD,
        kind,
        post_key: key,
        channel_id: CHANNEL,
        posted_at: at(10, minute),
    }
}

#[tokio::test]
async fn a_post_is_claimed_once_and_confirmed_later() {
    let db = test_db().await;
    let id = bot::claim_post(&db.pool, &post(PostKind::Digest, "2026-11-20", 0))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        bot::claim_post(&db.pool, &post(PostKind::Digest, "2026-11-20", 5))
            .await
            .unwrap(),
        None,
        "the same key again"
    );
    // The same key of another kind is another post.
    assert!(
        bot::claim_post(&db.pool, &post(PostKind::Results, "2026-11-20", 6))
            .await
            .unwrap()
            .is_some()
    );

    let unsent = bot::unsent(&db.pool, GUILD).await.unwrap();
    assert_eq!(unsent.len(), 2);
    assert_eq!(unsent[0].id, id);
    assert_eq!(unsent[0].message_id, None);

    bot::set_message(&db.pool, id, "900000000000000001", "h1")
        .await
        .unwrap();
    bot::set_edited(&db.pool, id, "h2", at(10, 30))
        .await
        .unwrap();
    let p = bot::get_post(&db.pool, GUILD, PostKind::Digest, "2026-11-20")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(p.message_id.as_deref(), Some("900000000000000001"));
    assert_eq!(p.content_hash.as_deref(), Some("h2"));
    assert_eq!(p.edited_at, Some(at(10, 30)));
    assert_eq!(p.posted_at, at(10, 0));
    assert_eq!(bot::unsent(&db.pool, GUILD).await.unwrap().len(), 1);

    // Counted for the rate limit by posting time, exclusive at the start.
    assert_eq!(
        bot::count_since(&db.pool, GUILD, at(9, 30)).await.unwrap(),
        2
    );
    assert_eq!(
        bot::count_since(&db.pool, GUILD, at(10, 0)).await.unwrap(),
        1
    );
    assert_eq!(
        bot::count_since(&db.pool, GUILD, at(10, 6)).await.unwrap(),
        0
    );

    // Bad values are rejected by the schema.
    for sql in [
        "INSERT INTO bot_posts (guild_id, kind, post_key, channel_id, posted_at) \
         VALUES ('1', 'reminder', 'k', '2', '2026-11-20T10:00:00.000Z')",
        "INSERT INTO bot_posts (guild_id, kind, post_key, channel_id, posted_at) \
         VALUES ('1', 'digest', 'k', 'chan', '2026-11-20T10:00:00.000Z')",
        "INSERT INTO bot_posts (guild_id, kind, post_key, channel_id, posted_at) \
         VALUES ('1', 'digest', '', '2', '2026-11-20T10:00:00.000Z')",
    ] {
        assert!(exec(&db.pool, sql).await.is_err(), "{sql}");
    }
}

#[tokio::test]
async fn results_are_announced_once_per_guild() {
    let db = test_db().await;
    let s = seed(&db.pool).await;
    guilds::follow_event(
        &db.pool,
        &FollowedEvent {
            guild_id: GUILD.into(),
            event_id: s.event,
            followed_by: ALICE.into(),
            followed_at: at(8, 0),
        },
    )
    .await
    .unwrap();
    let mut m = store::matches::get(&db.pool, s.bo3).await.unwrap().unwrap();
    m.fields.status = domain::MatchStatus::Finished;
    m.fields.ended_at = Some(at(20, 0));
    m.fields.maps_won_a = 2;
    m.fields.winner_team_id = Some(s.team_a);
    store::matches::update(&db.pool, &m).await.unwrap();

    let un = bot::unannounced_results(&db.pool, GUILD, None)
        .await
        .unwrap();
    assert_eq!(un.len(), 1);
    assert_eq!((un[0].match_id, un[0].finished_at), (s.bo3, at(20, 0)));
    // Not before `since`, and not in a guild that doesn't follow the event.
    assert!(
        bot::unannounced_results(&db.pool, GUILD, Some(at(20, 1)))
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        bot::unannounced_results(&db.pool, "100000000000000009", None)
            .await
            .unwrap()
            .is_empty()
    );

    let id = bot::claim_post(&db.pool, &post(PostKind::Results, "m1", 0))
        .await
        .unwrap()
        .unwrap();
    let mut conn = db.pool.acquire().await.unwrap();
    bot::claim_results(&mut conn, GUILD, id, &[s.bo3])
        .await
        .unwrap();
    // Claiming again is a no-op.
    bot::claim_results(&mut conn, GUILD, id, &[s.bo3])
        .await
        .unwrap();
    drop(conn);
    assert_eq!(bot::post_matches(&db.pool, id).await.unwrap(), vec![s.bo3]);
    assert!(
        bot::unannounced_results(&db.pool, GUILD, None)
            .await
            .unwrap()
            .is_empty()
    );

    // A voided match is never announced.
    m.fields.voided = true;
    store::matches::update(&db.pool, &m).await.unwrap();
    exec(&db.pool, "DELETE FROM announced_results")
        .await
        .unwrap();
    assert!(
        bot::unannounced_results(&db.pool, GUILD, None)
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn top_pickers_by_points_with_names() {
    let db = test_db().await;
    let s = seed(&db.pool).await;
    store::users::upsert_member(
        &db.pool,
        &store::users::GuildMember {
            guild_id: GUILD.into(),
            user_id: ALICE.into(),
            nick: Some("Ali".into()),
            last_seen_at: at(9, 0),
        },
    )
    .await
    .unwrap();
    let parts = PickParts {
        winner_team_id: Some(s.team_a),
        series: None,
        maps: Vec::new(),
    };
    let mut ids = Vec::new();
    for user in [ALICE, BOB] {
        let p = store::picks::save(&db.pool, GUILD, user, s.bo3, &parts, at(9, 0))
            .await
            .unwrap();
        ids.push(p.id);
    }
    for (pick_id, points) in ids.iter().zip([5, 0]) {
        store::picks::upsert_result(
            &db.pool,
            &store::picks::PickResult {
                pick_id: *pick_id,
                points,
                breakdown: serde_json::json!({}),
                rules_version: 1,
                settled_at: at(21, 0),
                state: domain::ResultState::Final,
                pending_parts: 0,
            },
        )
        .await
        .unwrap();
    }
    let top = bot::top_pickers(&db.pool, GUILD, s.bo3, 3).await.unwrap();
    assert_eq!(top.len(), 1, "zero points don't count: {top:?}");
    assert_eq!((top[0].display_name.as_str(), top[0].points), ("Ali", 5));

    let names = bot::display_names(&db.pool, GUILD, &[ALICE.into(), BOB.into()])
        .await
        .unwrap();
    assert_eq!(
        names,
        vec![
            (ALICE.to_owned(), "Ali".to_owned()),
            (BOB.to_owned(), format!("user{BOB}")),
        ]
    );
}

#[tokio::test]
async fn pending_views_are_taken_once_before_they_expire() {
    let db = test_db().await;
    let s = seed(&db.pool).await;
    let ttl = at(10, 0) + Duration::minutes(10);
    pending_views::put(&db.pool, GUILD, ALICE, s.bo3, ttl)
        .await
        .unwrap();
    assert_eq!(
        pending_views::take(&db.pool, GUILD, BOB, at(10, 1))
            .await
            .unwrap(),
        None
    );
    assert_eq!(
        pending_views::take(&db.pool, GUILD, ALICE, at(10, 1))
            .await
            .unwrap(),
        Some(s.bo3)
    );
    assert_eq!(
        pending_views::take(&db.pool, GUILD, ALICE, at(10, 1))
            .await
            .unwrap(),
        None
    );

    // Expired: gone and not returned.
    pending_views::put(&db.pool, GUILD, ALICE, s.bo3, ttl)
        .await
        .unwrap();
    assert_eq!(
        pending_views::take(&db.pool, GUILD, ALICE, at(10, 10))
            .await
            .unwrap(),
        None
    );
    pending_views::put(&db.pool, GUILD, ALICE, s.bo3, ttl)
        .await
        .unwrap();
    pending_views::put(&db.pool, GUILD, BOB, s.bo3, ttl + Duration::hours(1))
        .await
        .unwrap();
    assert_eq!(
        pending_views::purge_expired(&db.pool, at(10, 30))
            .await
            .unwrap(),
        1
    );

    // Only existing matches.
    assert!(
        pending_views::put(&db.pool, GUILD, ALICE, MatchId(9999), ttl)
            .await
            .is_err()
    );
}

#[tokio::test]
async fn bot_settings_change_one_field_and_keep_the_rest() {
    let db = test_db().await;
    let defaults = GuildSettings::new(GUILD, at(9, 0));
    assert_eq!(defaults.digest_time, "10:00");
    assert_eq!(defaults.timezone, "Europe/Berlin");

    guilds::set_announce_channel(&db.pool, GUILD, CHANNEL, at(9, 0))
        .await
        .unwrap();
    guilds::set_admin_role(&db.pool, GUILD, Some("400000000000000001"), at(9, 5))
        .await
        .unwrap();
    guilds::set_digest_schedule(&db.pool, GUILD, "08:30", "America/New_York", at(9, 10))
        .await
        .unwrap();
    store::scoring::set_rules(
        &db.pool,
        GUILD,
        &serde_json::json!({ "winner": 5 }),
        at(9, 15),
    )
    .await
    .unwrap();
    let s = guilds::get_settings(&db.pool, GUILD)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(s.announce_channel_id.as_deref(), Some(CHANNEL));
    assert_eq!(s.announce_since, Some(at(9, 0)));
    assert_eq!(s.admin_role_id.as_deref(), Some("400000000000000001"));
    assert_eq!(
        (s.digest_time.as_str(), s.timezone.as_str()),
        ("08:30", "America/New_York")
    );
    assert_eq!(s.scoring_rules_version, 2);

    guilds::set_admin_role(&db.pool, GUILD, None, at(9, 20))
        .await
        .unwrap();
    let listed = guilds::with_announce_channel(&db.pool).await.unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].admin_role_id, None);
    assert_eq!(listed[0].digest_time, "08:30");

    // A new channel restarts `announce_since`.
    guilds::set_announce_channel(&db.pool, GUILD, "300000000000000002", at(12, 0))
        .await
        .unwrap();
    let s = guilds::get_settings(&db.pool, GUILD)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(s.announce_since, Some(at(12, 0)));

    // The schema rejects malformed digest times.
    for t in ["24:00", "9:00", "10:60", "ten"] {
        assert!(
            guilds::set_digest_schedule(&db.pool, GUILD, t, "UTC", at(12, 0))
                .await
                .is_err(),
            "{t}"
        );
    }
}
