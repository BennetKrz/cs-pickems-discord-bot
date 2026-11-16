//! Append-only audit log of admin and manual changes.

use chrono::{DateTime, Utc};
use serde_json::Value;
use sqlx::sqlite::SqliteRow;
use sqlx::types::Json;
use sqlx::{Row, SqliteExecutor};

use crate::{DbResult, Ts};

/// One audited change.
#[derive(Debug, Clone, PartialEq)]
pub struct AuditRecord {
    /// A Discord user ID, or a component name such as `sync`.
    pub actor: String,
    pub action: String,
    pub entity_type: String,
    pub entity_id: Option<String>,
    pub before: Option<Value>,
    pub after: Option<Value>,
    pub at: DateTime<Utc>,
}

/// A stored [`AuditRecord`].
#[derive(Debug, Clone, PartialEq)]
pub struct AuditEntry {
    pub id: i64,
    pub record: AuditRecord,
}

fn from_row(row: SqliteRow) -> DbResult<AuditEntry> {
    let json = |col: &str| -> DbResult<Option<Value>> {
        Ok(row.try_get::<Option<Json<Value>>, _>(col)?.map(|j| j.0))
    };
    Ok(AuditEntry {
        id: row.try_get("id")?,
        record: AuditRecord {
            actor: row.try_get("actor")?,
            action: row.try_get("action")?,
            entity_type: row.try_get("entity_type")?,
            entity_id: row.try_get("entity_id")?,
            before: json("before")?,
            after: json("after")?,
            at: row.try_get("at")?,
        },
    })
}

/// Appends an entry and returns its ID.
pub async fn append(ex: impl SqliteExecutor<'_>, r: &AuditRecord) -> DbResult<i64> {
    sqlx::query_scalar(
        "INSERT INTO audit_log (actor, action, entity_type, entity_id, before, after, at) \
         VALUES (?, ?, ?, ?, ?, ?, ?) RETURNING id",
    )
    .bind(&r.actor)
    .bind(&r.action)
    .bind(&r.entity_type)
    .bind(&r.entity_id)
    .bind(r.before.as_ref().map(Json))
    .bind(r.after.as_ref().map(Json))
    .bind(Ts(r.at))
    .fetch_one(ex)
    .await
}

/// The history of one entity, oldest first.
pub async fn list_for_entity(
    ex: impl SqliteExecutor<'_>,
    entity_type: &str,
    entity_id: &str,
) -> DbResult<Vec<AuditEntry>> {
    sqlx::query(
        "SELECT id, actor, action, entity_type, entity_id, before, after, at FROM audit_log \
         WHERE entity_type = ? AND entity_id = ? ORDER BY id",
    )
    .bind(entity_type)
    .bind(entity_id)
    .try_map(from_row)
    .fetch_all(ex)
    .await
}
