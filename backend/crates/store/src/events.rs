//! Events and their stages.

use domain::{Event, EventId, NewEvent, NewStage, Stage, StageId};
use sqlx::sqlite::SqliteRow;
use sqlx::{Row, SqliteExecutor};

use crate::row::enum_col;
use crate::{DbResult, Ts};

macro_rules! event_columns {
    () => {
        "id, name, tier, begin_at, end_at, status"
    };
}
macro_rules! stage_columns {
    () => {
        "id, event_id, name, kind, position"
    };
}

fn event_from_row(row: SqliteRow) -> DbResult<Event> {
    Ok(Event {
        id: EventId(row.try_get("id")?),
        name: row.try_get("name")?,
        tier: enum_col(&row, "tier")?,
        begin_at: row.try_get("begin_at")?,
        end_at: row.try_get("end_at")?,
        status: enum_col(&row, "status")?,
    })
}

fn stage_from_row(row: SqliteRow) -> DbResult<Stage> {
    Ok(Stage {
        id: StageId(row.try_get("id")?),
        event_id: EventId(row.try_get("event_id")?),
        name: row.try_get("name")?,
        kind: enum_col(&row, "kind")?,
        position: row.try_get("position")?,
    })
}

pub async fn insert(ex: impl SqliteExecutor<'_>, event: &NewEvent) -> DbResult<EventId> {
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO events (name, tier, begin_at, end_at, status) VALUES (?, ?, ?, ?, ?) \
         RETURNING id",
    )
    .bind(&event.name)
    .bind(event.tier.as_str())
    .bind(Ts::opt(event.begin_at))
    .bind(Ts::opt(event.end_at))
    .bind(event.status.as_str())
    .fetch_one(ex)
    .await?;
    Ok(EventId(id))
}

/// Overwrites every field of an existing event. Returns `false` if it does
/// not exist.
pub async fn update(ex: impl SqliteExecutor<'_>, event: &Event) -> DbResult<bool> {
    let res = sqlx::query(
        "UPDATE events SET name = ?, tier = ?, begin_at = ?, end_at = ?, status = ? WHERE id = ?",
    )
    .bind(&event.name)
    .bind(event.tier.as_str())
    .bind(Ts::opt(event.begin_at))
    .bind(Ts::opt(event.end_at))
    .bind(event.status.as_str())
    .bind(event.id.0)
    .execute(ex)
    .await?;
    Ok(res.rows_affected() == 1)
}

pub async fn get(ex: impl SqliteExecutor<'_>, id: EventId) -> DbResult<Option<Event>> {
    sqlx::query(concat!(
        "SELECT ",
        event_columns!(),
        " FROM events WHERE id = ?"
    ))
    .bind(id.0)
    .try_map(event_from_row)
    .fetch_optional(ex)
    .await
}

pub async fn insert_stage(ex: impl SqliteExecutor<'_>, stage: &NewStage) -> DbResult<StageId> {
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO stages (event_id, name, kind, position) VALUES (?, ?, ?, ?) RETURNING id",
    )
    .bind(stage.event_id.0)
    .bind(&stage.name)
    .bind(stage.kind.as_str())
    .bind(stage.position)
    .fetch_one(ex)
    .await?;
    Ok(StageId(id))
}

/// Overwrites every field of an existing stage. Returns `false` if it does
/// not exist.
pub async fn update_stage(ex: impl SqliteExecutor<'_>, stage: &Stage) -> DbResult<bool> {
    let res = sqlx::query(
        "UPDATE stages SET event_id = ?, name = ?, kind = ?, position = ? WHERE id = ?",
    )
    .bind(stage.event_id.0)
    .bind(&stage.name)
    .bind(stage.kind.as_str())
    .bind(stage.position)
    .bind(stage.id.0)
    .execute(ex)
    .await?;
    Ok(res.rows_affected() == 1)
}

pub async fn get_stage(ex: impl SqliteExecutor<'_>, id: StageId) -> DbResult<Option<Stage>> {
    sqlx::query(concat!(
        "SELECT ",
        stage_columns!(),
        " FROM stages WHERE id = ?"
    ))
    .bind(id.0)
    .try_map(stage_from_row)
    .fetch_optional(ex)
    .await
}

/// The stages of an event, in order.
pub async fn list_stages(ex: impl SqliteExecutor<'_>, event_id: EventId) -> DbResult<Vec<Stage>> {
    sqlx::query(concat!(
        "SELECT ",
        stage_columns!(),
        " FROM stages WHERE event_id = ? ORDER BY position, id"
    ))
    .bind(event_id.0)
    .try_map(stage_from_row)
    .fetch_all(ex)
    .await
}
