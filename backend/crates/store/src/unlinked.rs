//! The unlinked queue (`unlinked_refs`): external entities that could not be
//! matched to a canonical entity and wait for an admin (T03, T08).

use chrono::{DateTime, Utc};
use domain::{EntityType, UnlinkedReason};
use sqlx::sqlite::SqliteRow;
use sqlx::{Row, SqliteExecutor};

use crate::row::enum_col;
use crate::{DbResult, Ts};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnlinkedRef {
    pub source: String,
    pub entity_type: EntityType,
    pub external_id: String,
    pub reason: UnlinkedReason,
    pub detail: Option<String>,
    pub first_seen_at: DateTime<Utc>,
}

fn from_row(row: SqliteRow) -> DbResult<UnlinkedRef> {
    Ok(UnlinkedRef {
        source: row.try_get("source")?,
        entity_type: enum_col(&row, "entity_type")?,
        external_id: row.try_get("external_id")?,
        reason: enum_col(&row, "reason")?,
        detail: row.try_get("detail")?,
        first_seen_at: row.try_get("first_seen_at")?,
    })
}

/// Queues an entity, or updates its reason and detail if already queued.
/// `first_seen_at` is kept from the first time.
pub async fn upsert(ex: impl SqliteExecutor<'_>, u: &UnlinkedRef) -> DbResult<()> {
    sqlx::query(
        "INSERT INTO unlinked_refs \
           (source, entity_type, external_id, reason, detail, first_seen_at) \
         VALUES (?, ?, ?, ?, ?, ?) \
         ON CONFLICT (source, entity_type, external_id) DO UPDATE SET \
           reason = excluded.reason, detail = excluded.detail \
         WHERE reason <> excluded.reason OR detail IS NOT excluded.detail",
    )
    .bind(&u.source)
    .bind(u.entity_type.as_str())
    .bind(&u.external_id)
    .bind(u.reason.as_str())
    .bind(&u.detail)
    .bind(Ts(u.first_seen_at))
    .execute(ex)
    .await?;
    Ok(())
}

/// Removes an entity from the queue. Returns whether it was queued.
pub async fn remove(
    ex: impl SqliteExecutor<'_>,
    source: &str,
    entity_type: EntityType,
    external_id: &str,
) -> DbResult<bool> {
    let res = sqlx::query(
        "DELETE FROM unlinked_refs WHERE source = ? AND entity_type = ? AND external_id = ?",
    )
    .bind(source)
    .bind(entity_type.as_str())
    .bind(external_id)
    .execute(ex)
    .await?;
    Ok(res.rows_affected() == 1)
}

pub async fn get(
    ex: impl SqliteExecutor<'_>,
    source: &str,
    entity_type: EntityType,
    external_id: &str,
) -> DbResult<Option<UnlinkedRef>> {
    sqlx::query(
        "SELECT source, entity_type, external_id, reason, detail, first_seen_at \
         FROM unlinked_refs WHERE source = ? AND entity_type = ? AND external_id = ?",
    )
    .bind(source)
    .bind(entity_type.as_str())
    .bind(external_id)
    .try_map(from_row)
    .fetch_optional(ex)
    .await
}

/// The queue, oldest first; only one source's entries if `source` is set.
pub async fn list(ex: impl SqliteExecutor<'_>, source: Option<&str>) -> DbResult<Vec<UnlinkedRef>> {
    sqlx::query(
        "SELECT source, entity_type, external_id, reason, detail, first_seen_at \
         FROM unlinked_refs WHERE ?1 IS NULL OR source = ?1 \
         ORDER BY first_seen_at, source, entity_type, external_id",
    )
    .bind(source)
    .try_map(from_row)
    .fetch_all(ex)
    .await
}
