//! Field provenance (`field_sources`): which source set each current value
//! of a canonical entity (T03).

use chrono::{DateTime, Utc};
use domain::EntityType;
use sqlx::sqlite::SqliteRow;
use sqlx::{Row, SqliteExecutor};

use crate::row::enum_col;
use crate::{DbResult, Ts};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FieldSource {
    pub entity_type: EntityType,
    pub entity_id: i64,
    /// Field group, e.g. `schedule` or `map_rounds.2`.
    pub field: String,
    pub source: String,
    /// When `source` last changed for this field.
    pub updated_at: DateTime<Utc>,
}

fn from_row(row: SqliteRow) -> DbResult<FieldSource> {
    Ok(FieldSource {
        entity_type: enum_col(&row, "entity_type")?,
        entity_id: row.try_get("entity_id")?,
        field: row.try_get("field")?,
        source: row.try_get("source")?,
        updated_at: row.try_get("updated_at")?,
    })
}

/// Records `source` for a field. A no-op if it is already the source, so
/// `updated_at` only moves on a real change.
pub async fn set(
    ex: impl SqliteExecutor<'_>,
    entity_type: EntityType,
    entity_id: i64,
    field: &str,
    source: &str,
    now: DateTime<Utc>,
) -> DbResult<()> {
    sqlx::query(
        "INSERT INTO field_sources (entity_type, entity_id, field, source, updated_at) \
         VALUES (?, ?, ?, ?, ?) \
         ON CONFLICT (entity_type, entity_id, field) DO UPDATE SET \
           source = excluded.source, updated_at = excluded.updated_at \
         WHERE source <> excluded.source",
    )
    .bind(entity_type.as_str())
    .bind(entity_id)
    .bind(field)
    .bind(source)
    .bind(Ts(now))
    .execute(ex)
    .await?;
    Ok(())
}

/// Forgets the source of a field (no source has a value any more).
pub async fn clear(
    ex: impl SqliteExecutor<'_>,
    entity_type: EntityType,
    entity_id: i64,
    field: &str,
) -> DbResult<()> {
    sqlx::query("DELETE FROM field_sources WHERE entity_type = ? AND entity_id = ? AND field = ?")
        .bind(entity_type.as_str())
        .bind(entity_id)
        .bind(field)
        .execute(ex)
        .await?;
    Ok(())
}

/// All field sources of one entity, by field.
pub async fn list_for(
    ex: impl SqliteExecutor<'_>,
    entity_type: EntityType,
    entity_id: i64,
) -> DbResult<Vec<FieldSource>> {
    sqlx::query(
        "SELECT entity_type, entity_id, field, source, updated_at FROM field_sources \
         WHERE entity_type = ? AND entity_id = ? ORDER BY field",
    )
    .bind(entity_type.as_str())
    .bind(entity_id)
    .try_map(from_row)
    .fetch_all(ex)
    .await
}
