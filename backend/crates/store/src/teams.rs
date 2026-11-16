//! Teams.

use domain::{NewTeam, Team, TeamId};
use sqlx::sqlite::SqliteRow;
use sqlx::{Row, SqliteExecutor};

use crate::DbResult;

macro_rules! columns {
    () => {
        "id, name, acronym, logo_url, country"
    };
}

fn from_row(row: SqliteRow) -> DbResult<Team> {
    Ok(Team {
        id: TeamId(row.try_get("id")?),
        name: row.try_get("name")?,
        acronym: row.try_get("acronym")?,
        logo_url: row.try_get("logo_url")?,
        country: row.try_get("country")?,
    })
}

pub async fn insert(ex: impl SqliteExecutor<'_>, team: &NewTeam) -> DbResult<TeamId> {
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO teams (name, acronym, logo_url, country) VALUES (?, ?, ?, ?) RETURNING id",
    )
    .bind(&team.name)
    .bind(&team.acronym)
    .bind(&team.logo_url)
    .bind(&team.country)
    .fetch_one(ex)
    .await?;
    Ok(TeamId(id))
}

/// Overwrites every field of an existing team. Returns `false` if it does
/// not exist.
pub async fn update(ex: impl SqliteExecutor<'_>, team: &Team) -> DbResult<bool> {
    let res = sqlx::query(
        "UPDATE teams SET name = ?, acronym = ?, logo_url = ?, country = ? WHERE id = ?",
    )
    .bind(&team.name)
    .bind(&team.acronym)
    .bind(&team.logo_url)
    .bind(&team.country)
    .bind(team.id.0)
    .execute(ex)
    .await?;
    Ok(res.rows_affected() == 1)
}

pub async fn get(ex: impl SqliteExecutor<'_>, id: TeamId) -> DbResult<Option<Team>> {
    sqlx::query(concat!("SELECT ", columns!(), " FROM teams WHERE id = ?"))
        .bind(id.0)
        .try_map(from_row)
        .fetch_optional(ex)
        .await
}

/// All teams, by name. For matching during reconciliation (T03).
pub async fn list(ex: impl SqliteExecutor<'_>) -> DbResult<Vec<Team>> {
    sqlx::query(concat!(
        "SELECT ",
        columns!(),
        " FROM teams ORDER BY name, id"
    ))
    .try_map(from_row)
    .fetch_all(ex)
    .await
}
