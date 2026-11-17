//! Repositories and constraints of migration 0002 (T03): field provenance,
//! the unlinked queue, and the reconciler's helper queries.

mod common;

use common::*;
use domain::{EntityType, MatchFormat, UnlinkedReason};
use store::refs::SourceRecord;
use store::unlinked::UnlinkedRef;
use store::{matches, provenance, refs, unlinked};

#[tokio::test]
async fn provenance_moves_only_on_a_real_change() {
    let db = test_db().await;
    let s = seed(&db.pool).await;
    let id = s.bo3.0;

    provenance::set(
        &db.pool,
        EntityType::Match,
        id,
        "schedule",
        "alpha",
        at(9, 0),
    )
    .await
    .unwrap();
    provenance::set(
        &db.pool,
        EntityType::Match,
        id,
        "schedule",
        "alpha",
        at(9, 5),
    )
    .await
    .unwrap();
    provenance::set(
        &db.pool,
        EntityType::Match,
        id,
        "map_rounds.1",
        "beta",
        at(9, 5),
    )
    .await
    .unwrap();
    let rows = provenance::list_for(&db.pool, EntityType::Match, id)
        .await
        .unwrap();
    let flat: Vec<_> = rows
        .iter()
        .map(|r| (r.field.as_str(), r.source.as_str(), r.updated_at))
        .collect();
    assert_eq!(
        flat,
        vec![
            ("map_rounds.1", "beta", at(9, 5)),
            ("schedule", "alpha", at(9, 0))
        ],
        "same source again keeps updated_at"
    );

    provenance::set(
        &db.pool,
        EntityType::Match,
        id,
        "schedule",
        "manual",
        at(9, 10),
    )
    .await
    .unwrap();
    provenance::clear(&db.pool, EntityType::Match, id, "map_rounds.1")
        .await
        .unwrap();
    let rows = provenance::list_for(&db.pool, EntityType::Match, id)
        .await
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(
        (rows[0].source.as_str(), rows[0].updated_at),
        ("manual", at(9, 10))
    );
    assert!(
        provenance::list_for(&db.pool, EntityType::Team, id)
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn unlinked_queue_keeps_first_seen() {
    let db = test_db().await;
    let mut entry = UnlinkedRef {
        source: "beta".into(),
        entity_type: EntityType::Team,
        external_id: "b-1".into(),
        reason: UnlinkedReason::NoMatch,
        detail: Some("team \"X\"".into()),
        first_seen_at: at(9, 0),
    };
    unlinked::upsert(&db.pool, &entry).await.unwrap();
    entry.reason = UnlinkedReason::Ambiguous;
    entry.first_seen_at = at(10, 0);
    unlinked::upsert(&db.pool, &entry).await.unwrap();
    let other = UnlinkedRef {
        source: "gamma".into(),
        external_id: "g-1".into(),
        first_seen_at: at(8, 0),
        ..entry.clone()
    };
    unlinked::upsert(&db.pool, &other).await.unwrap();

    let got = unlinked::get(&db.pool, "beta", EntityType::Team, "b-1")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        (got.reason, got.first_seen_at),
        (UnlinkedReason::Ambiguous, at(9, 0))
    );
    let all = unlinked::list(&db.pool, None).await.unwrap();
    assert_eq!(
        all.iter().map(|u| u.source.as_str()).collect::<Vec<_>>(),
        vec!["gamma", "beta"]
    );
    assert_eq!(
        unlinked::list(&db.pool, Some("beta")).await.unwrap().len(),
        1
    );

    assert!(
        unlinked::remove(&db.pool, "beta", EntityType::Team, "b-1")
            .await
            .unwrap()
    );
    assert!(
        !unlinked::remove(&db.pool, "beta", EntityType::Team, "b-1")
            .await
            .unwrap()
    );
    assert!(
        unlinked::get(&db.pool, "beta", EntityType::Team, "b-1")
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn new_tables_check_their_values() {
    let db = test_db().await;
    let ts = "'2026-11-20T09:00:00.000Z'";
    assert_rejected(
        exec(
            &db.pool,
            format!(
                "INSERT INTO unlinked_refs VALUES ('beta', 'team', 'x', 'unknown', NULL, {ts})"
            ),
        )
        .await,
        "CHECK constraint failed",
    );
    assert_rejected(
        exec(
            &db.pool,
            format!(
                "INSERT INTO unlinked_refs VALUES ('beta', 'player', 'x', 'no_match', NULL, {ts})"
            ),
        )
        .await,
        "CHECK constraint failed",
    );
    assert_rejected(
        exec(
            &db.pool,
            format!("INSERT INTO field_sources VALUES ('match', 1, '', 'alpha', {ts})"),
        )
        .await,
        "CHECK constraint failed",
    );
    assert_rejected(
        exec(&db.pool, "INSERT INTO field_sources VALUES ('match', 1, 'schedule', 'alpha', '2026-11-20 09:00')").await,
        "CHECK constraint failed",
    );
    // Every UnlinkedReason the domain knows is accepted.
    for (i, reason) in UnlinkedReason::ALL.iter().enumerate() {
        exec(
            &db.pool,
            format!(
                "INSERT INTO unlinked_refs VALUES ('beta', 'team', 'r{i}', '{reason}', NULL, {ts})"
            ),
        )
        .await
        .unwrap();
    }
}

#[tokio::test]
async fn find_between_matches_either_team_order_within_the_window() {
    let db = test_db().await;
    let s = seed(&db.pool).await;
    // A second match between the same teams, later, and one with team C.
    let later = match_between(
        &db.pool,
        s.event,
        Some(s.team_b),
        Some(s.team_a),
        MatchFormat::Bo1,
    )
    .await;
    let mut m = matches::get(&db.pool, later).await.unwrap().unwrap();
    m.fields.scheduled_at = Some(at(19, 30));
    matches::update(&db.pool, &m).await.unwrap();
    match_between(
        &db.pool,
        s.event,
        Some(s.team_a),
        Some(s.team_c),
        MatchFormat::Bo1,
    )
    .await;

    let found = matches::find_between(&db.pool, s.event, s.team_b, s.team_a, at(17, 0), at(19, 0))
        .await
        .unwrap();
    assert_eq!(found.iter().map(|m| m.id).collect::<Vec<_>>(), vec![s.bo3]);
    let found = matches::find_between(&db.pool, s.event, s.team_a, s.team_b, at(18, 0), at(20, 0))
        .await
        .unwrap();
    assert_eq!(
        found.iter().map(|m| m.id).collect::<Vec<_>>(),
        vec![later, s.bo3],
        "bounds are inclusive; closest to the window's middle (19:00) first"
    );
}

#[tokio::test]
async fn list_all_records_spans_sources() {
    let db = test_db().await;
    for (source, ext) in [("beta", "2"), ("alpha", "1"), ("alpha", "0")] {
        refs::upsert_record(
            &db.pool,
            &SourceRecord {
                source: source.into(),
                entity_type: EntityType::Team,
                external_id: ext.into(),
                payload: "{}".into(),
                payload_hash: "h".into(),
                fetched_at: at(9, 0),
            },
        )
        .await
        .unwrap();
    }
    let all = refs::list_all_records(&db.pool, EntityType::Team)
        .await
        .unwrap();
    let keys: Vec<_> = all
        .iter()
        .map(|r| (r.source.as_str(), r.external_id.as_str()))
        .collect();
    assert_eq!(keys, vec![("alpha", "0"), ("alpha", "1"), ("beta", "2")]);
    assert!(
        refs::list_all_records(&db.pool, EntityType::Match)
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn manual_links_and_records_can_be_removed() {
    let db = test_db().await;
    let s = seed(&db.pool).await;
    let ext = s.bo3.0.to_string();
    refs::link(
        &db.pool,
        &refs::ExternalRef {
            entity_type: EntityType::Match,
            entity_id: s.bo3.0,
            source: "manual".into(),
            external_id: ext.clone(),
        },
    )
    .await
    .unwrap();
    refs::upsert_record(
        &db.pool,
        &SourceRecord {
            source: "manual".into(),
            entity_type: EntityType::Match,
            external_id: ext.clone(),
            payload: "{}".into(),
            payload_hash: "h".into(),
            fetched_at: at(9, 0),
        },
    )
    .await
    .unwrap();

    assert!(
        refs::unlink(&db.pool, "manual", EntityType::Match, &ext)
            .await
            .unwrap()
    );
    assert!(
        refs::delete_record(&db.pool, "manual", EntityType::Match, &ext)
            .await
            .unwrap()
    );
    assert_eq!(
        refs::resolve(&db.pool, "manual", EntityType::Match, &ext)
            .await
            .unwrap(),
        None
    );
    assert!(
        refs::get_record(&db.pool, "manual", EntityType::Match, &ext)
            .await
            .unwrap()
            .is_none()
    );
    // Removing again is a no-op.
    assert!(
        !refs::unlink(&db.pool, "manual", EntityType::Match, &ext)
            .await
            .unwrap()
    );
    assert!(
        !refs::delete_record(&db.pool, "manual", EntityType::Match, &ext)
            .await
            .unwrap()
    );
}

#[tokio::test]
async fn voided_is_a_boolean() {
    let db = test_db().await;
    let s = seed(&db.pool).await;
    assert_rejected(
        exec(
            &db.pool,
            format!("UPDATE matches SET voided = 2 WHERE id = {}", s.bo3.0),
        )
        .await,
        "CHECK constraint failed",
    );
    let m = matches::get(&db.pool, s.bo3).await.unwrap().unwrap();
    assert!(!m.fields.voided, "defaults to not voided");
}
