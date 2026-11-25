//! *Pick now* (T08 AC 3, the server half): a click on an *open match*
//! button stores the pending view the Activity takes on start; the plain
//! launch button and unknown matches store nothing.

use bot::launch::remember_view;
use bot::render::ButtonAction;
use chrono::{Duration, TimeZone, Utc};
use domain::{EventStatus, EventTier, MatchFields, MatchFormat, MatchId, NewEvent};

const GUILD: &str = "100000000000000001";
const ALICE: &str = "200000000000000001";

#[tokio::test]
async fn open_buttons_store_a_pending_view_for_ten_minutes() {
    let dir = tempfile::tempdir().unwrap();
    let url = format!("sqlite://{}", dir.path().join("test.db").display());
    let pool = store::open(&url).await.unwrap();
    let now = Utc.with_ymd_and_hms(2026, 11, 20, 12, 0, 0).unwrap();
    let event = store::events::insert(
        &pool,
        &NewEvent {
            name: "Cup".into(),
            tier: EventTier::A,
            begin_at: None,
            end_at: None,
            status: EventStatus::Running,
        },
    )
    .await
    .unwrap();
    let m = store::matches::insert(
        &pool,
        &MatchFields::scheduled(event, None, None, MatchFormat::Bo3, Some(now)),
    )
    .await
    .unwrap();

    let action = ButtonAction::parse(&format!("open:{}", m.0)).unwrap();
    assert!(
        remember_view(&pool, GUILD, ALICE, action, now)
            .await
            .unwrap()
    );
    // The Activity (via GET /api/me/pending-view) gets it within 10 min.
    let later = now + Duration::minutes(9);
    assert_eq!(
        store::pending_views::take(&pool, GUILD, ALICE, later)
            .await
            .unwrap(),
        Some(m)
    );

    // Plain launch: nothing to remember.
    assert!(
        !remember_view(&pool, GUILD, ALICE, ButtonAction::Launch, now)
            .await
            .unwrap()
    );
    // A match that doesn't exist: nothing either, and no error.
    let gone = ButtonAction::Open(MatchId(m.0 + 100));
    assert!(!remember_view(&pool, GUILD, ALICE, gone, now).await.unwrap());
    assert_eq!(
        store::pending_views::take(&pool, GUILD, ALICE, now)
            .await
            .unwrap(),
        None
    );

    // After 10 minutes the view has expired.
    remember_view(&pool, GUILD, ALICE, action, now)
        .await
        .unwrap();
    let too_late = now + Duration::minutes(10);
    assert_eq!(
        store::pending_views::take(&pool, GUILD, ALICE, too_late)
            .await
            .unwrap(),
        None
    );
}

/// A global command as Discord's `GET /applications/{id}/commands` returns it.
fn command(id: u64, kind: u8, handler: Option<u8>) -> serde_json::Value {
    let mut c = serde_json::json!({
        "id": id.to_string(),
        "application_id": "300000000000000001",
        "version": "400000000000000001",
        "default_member_permissions": null,
        "type": kind,
        "name": if kind == 4 { "launch" } else { "other" },
        "description": "",
        "dm_permission": true,
        "contexts": [0, 1, 2],
        "integration_types": [0],
        "nsfw": false,
    });
    if let Some(h) = handler {
        c["handler"] = h.into();
    }
    c
}

fn commands(json: Vec<serde_json::Value>) -> Vec<poise::serenity_prelude::Command> {
    serde_json::from_value(serde_json::Value::Array(json)).unwrap()
}

/// The App Launcher's Entry Point command is switched to the bot's handler
/// once, so Discord stops posting a message on each launch (ADR 0019).
#[test]
fn only_an_entry_point_left_to_discord_is_claimed() {
    use bot::launch::entry_point_to_claim;

    // Discord's default: DISCORD_LAUNCH_ACTIVITY (2) → switch it.
    let default = commands(vec![command(1, 1, None), command(2, 4, Some(2))]);
    assert_eq!(entry_point_to_claim(&default).map(|id| id.get()), Some(2));
    // Already APP_HANDLER (1) → nothing to do.
    let claimed = commands(vec![command(2, 4, Some(1))]);
    assert_eq!(entry_point_to_claim(&claimed), None);
    // No Entry Point command (Activities off) → nothing to do.
    let none = commands(vec![command(1, 1, None)]);
    assert_eq!(entry_point_to_claim(&none), None);
}
