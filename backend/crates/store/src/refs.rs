//! External IDs (`external_refs`) and raw source payloads (`source_records`).
//!
//! External IDs never leave the backend; the public API uses internal IDs.

use chrono::{DateTime, Utc};
use domain::EntityType;
use sqlx::sqlite::SqliteRow;
use sqlx::{Acquire, Row, Sqlite, SqliteExecutor};

use crate::row::enum_col;
use crate::{DbResult, Ts};

/// One external ID of a canonical entity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExternalRef {
    pub entity_type: EntityType,
    pub entity_id: i64,
    pub source: String,
    pub external_id: String,
}

/// The latest raw payload a source returned for one external entity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceRecord {
    pub source: String,
    pub entity_type: EntityType,
    pub external_id: String,
    /// Raw JSON as received.
    pub payload: String,
    pub payload_hash: String,
    pub fetched_at: DateTime<Utc>,
}

/// What [`upsert_record`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecordChange {
    Inserted,
    /// The hash differed; the payload was replaced.
    Changed,
    /// Same hash; only `fetched_at` moved.
    Unchanged,
}

fn ref_from_row(row: SqliteRow) -> DbResult<ExternalRef> {
    Ok(ExternalRef {
        entity_type: enum_col(&row, "entity_type")?,
        entity_id: row.try_get("entity_id")?,
        source: row.try_get("source")?,
        external_id: row.try_get("external_id")?,
    })
}

fn record_from_row(row: SqliteRow) -> DbResult<SourceRecord> {
    Ok(SourceRecord {
        source: row.try_get("source")?,
        entity_type: enum_col(&row, "entity_type")?,
        external_id: row.try_get("external_id")?,
        payload: row.try_get("payload")?,
        payload_hash: row.try_get("payload_hash")?,
        fetched_at: row.try_get("fetched_at")?,
    })
}

/// Records a new external ID. Fails with a unique violation if this source
/// already maps the external ID to an entity; use [`relink`] to move it.
pub async fn link(ex: impl SqliteExecutor<'_>, r: &ExternalRef) -> DbResult<()> {
    sqlx::query(
        "INSERT INTO external_refs (entity_type, entity_id, source, external_id) \
         VALUES (?, ?, ?, ?)",
    )
    .bind(r.entity_type.as_str())
    .bind(r.entity_id)
    .bind(&r.source)
    .bind(&r.external_id)
    .execute(ex)
    .await?;
    Ok(())
}

/// Points an external ID at `r.entity_id`, creating the link if missing.
/// Used when admins merge duplicates (T03).
pub async fn relink(ex: impl SqliteExecutor<'_>, r: &ExternalRef) -> DbResult<()> {
    sqlx::query(
        "INSERT INTO external_refs (entity_type, entity_id, source, external_id) \
         VALUES (?, ?, ?, ?) \
         ON CONFLICT (source, entity_type, external_id) DO UPDATE SET entity_id = excluded.entity_id",
    )
    .bind(r.entity_type.as_str())
    .bind(r.entity_id)
    .bind(&r.source)
    .bind(&r.external_id)
    .execute(ex)
    .await?;
    Ok(())
}

/// The canonical entity ID for an external ID, if linked.
pub async fn resolve(
    ex: impl SqliteExecutor<'_>,
    source: &str,
    entity_type: EntityType,
    external_id: &str,
) -> DbResult<Option<i64>> {
    sqlx::query_scalar(
        "SELECT entity_id FROM external_refs \
         WHERE source = ? AND entity_type = ? AND external_id = ?",
    )
    .bind(source)
    .bind(entity_type.as_str())
    .bind(external_id)
    .fetch_optional(ex)
    .await
}

/// All external IDs of one entity, by source.
pub async fn refs_of(
    ex: impl SqliteExecutor<'_>,
    entity_type: EntityType,
    entity_id: i64,
) -> DbResult<Vec<ExternalRef>> {
    sqlx::query(
        "SELECT entity_type, entity_id, source, external_id FROM external_refs \
         WHERE entity_type = ? AND entity_id = ? ORDER BY source, external_id",
    )
    .bind(entity_type.as_str())
    .bind(entity_id)
    .try_map(ref_from_row)
    .fetch_all(ex)
    .await
}

/// The external IDs one source has for the matches of an event, as
/// `(match ID, external ID)` pairs ordered by match. For the sync scheduler
/// (T06), which asks each source only for matches it knows.
pub async fn match_refs_for_event(
    ex: impl SqliteExecutor<'_>,
    source: &str,
    event_id: i64,
) -> DbResult<Vec<(i64, String)>> {
    sqlx::query_as(
        "SELECT r.entity_id, r.external_id FROM external_refs r \
         JOIN matches m ON m.id = r.entity_id \
         WHERE r.source = ? AND r.entity_type = 'match' AND m.event_id = ? \
         ORDER BY r.entity_id, r.external_id",
    )
    .bind(source)
    .bind(event_id)
    .fetch_all(ex)
    .await
}

/// Stores the latest payload for an external entity. The payload is only
/// rewritten when its hash changed; `fetched_at` always moves.
pub async fn upsert_record<'a, A>(conn: A, rec: &SourceRecord) -> DbResult<RecordChange>
where
    A: Acquire<'a, Database = Sqlite>,
{
    let mut tx = conn.begin().await?;
    let old_hash: Option<String> = sqlx::query_scalar(
        "SELECT payload_hash FROM source_records \
         WHERE source = ? AND entity_type = ? AND external_id = ?",
    )
    .bind(&rec.source)
    .bind(rec.entity_type.as_str())
    .bind(&rec.external_id)
    .fetch_optional(&mut *tx)
    .await?;

    let change = match old_hash {
        None => RecordChange::Inserted,
        Some(h) if h == rec.payload_hash => RecordChange::Unchanged,
        Some(_) => RecordChange::Changed,
    };

    if change == RecordChange::Unchanged {
        sqlx::query(
            "UPDATE source_records SET fetched_at = ? \
             WHERE source = ? AND entity_type = ? AND external_id = ?",
        )
        .bind(Ts(rec.fetched_at))
        .bind(&rec.source)
        .bind(rec.entity_type.as_str())
        .bind(&rec.external_id)
        .execute(&mut *tx)
        .await?;
    } else {
        sqlx::query(
            "INSERT INTO source_records \
               (source, entity_type, external_id, payload, payload_hash, fetched_at) \
             VALUES (?, ?, ?, ?, ?, ?) \
             ON CONFLICT (source, entity_type, external_id) DO UPDATE SET \
               payload = excluded.payload, payload_hash = excluded.payload_hash, \
               fetched_at = excluded.fetched_at",
        )
        .bind(&rec.source)
        .bind(rec.entity_type.as_str())
        .bind(&rec.external_id)
        .bind(&rec.payload)
        .bind(&rec.payload_hash)
        .bind(Ts(rec.fetched_at))
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    Ok(change)
}

pub async fn get_record(
    ex: impl SqliteExecutor<'_>,
    source: &str,
    entity_type: EntityType,
    external_id: &str,
) -> DbResult<Option<SourceRecord>> {
    sqlx::query(
        "SELECT source, entity_type, external_id, payload, payload_hash, fetched_at \
         FROM source_records WHERE source = ? AND entity_type = ? AND external_id = ?",
    )
    .bind(source)
    .bind(entity_type.as_str())
    .bind(external_id)
    .try_map(record_from_row)
    .fetch_optional(ex)
    .await
}

/// All records of one entity type across every source, by source and
/// external ID. For replay (T03).
pub async fn list_all_records(
    ex: impl SqliteExecutor<'_>,
    entity_type: EntityType,
) -> DbResult<Vec<SourceRecord>> {
    sqlx::query(
        "SELECT source, entity_type, external_id, payload, payload_hash, fetched_at \
         FROM source_records WHERE entity_type = ? ORDER BY source, external_id",
    )
    .bind(entity_type.as_str())
    .try_map(record_from_row)
    .fetch_all(ex)
    .await
}

/// All records of one source and entity type, for replay (T03).
pub async fn list_records(
    ex: impl SqliteExecutor<'_>,
    source: &str,
    entity_type: EntityType,
) -> DbResult<Vec<SourceRecord>> {
    sqlx::query(
        "SELECT source, entity_type, external_id, payload, payload_hash, fetched_at \
         FROM source_records WHERE source = ? AND entity_type = ? ORDER BY external_id",
    )
    .bind(source)
    .bind(entity_type.as_str())
    .try_map(record_from_row)
    .fetch_all(ex)
    .await
}

/// Removes a link. Returns `false` if it did not exist. Used when the last
/// manual override of an entity is cleared (T05).
pub async fn unlink(
    ex: impl SqliteExecutor<'_>,
    source: &str,
    entity_type: EntityType,
    external_id: &str,
) -> DbResult<bool> {
    let res = sqlx::query(
        "DELETE FROM external_refs WHERE source = ? AND entity_type = ? AND external_id = ?",
    )
    .bind(source)
    .bind(entity_type.as_str())
    .bind(external_id)
    .execute(ex)
    .await?;
    Ok(res.rows_affected() == 1)
}

/// Deletes a stored record. Returns `false` if it did not exist. Only for
/// sources whose records are not fetched data, i.e. `manual` (T05).
pub async fn delete_record(
    ex: impl SqliteExecutor<'_>,
    source: &str,
    entity_type: EntityType,
    external_id: &str,
) -> DbResult<bool> {
    let res = sqlx::query(
        "DELETE FROM source_records WHERE source = ? AND entity_type = ? AND external_id = ?",
    )
    .bind(source)
    .bind(entity_type.as_str())
    .bind(external_id)
    .execute(ex)
    .await?;
    Ok(res.rows_affected() == 1)
}
