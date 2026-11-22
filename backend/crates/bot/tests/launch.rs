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
