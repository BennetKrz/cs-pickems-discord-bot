//! The pick API (T10). Rules are in `domain::picks`; this module only
//! translates HTTP and runs them against the database.
//!
//! - `PUT /api/matches/{id}/pick` creates or replaces the caller's whole
//!   pick; a pick without parts deletes it (204).
//! - `DELETE /api/matches/{id}/pick` deletes it (204, also if there was none).
//! - `GET /api/matches/{id}/picks` lists every pick of the guild for the
//!   match with display names and settled points (T12), plus aggregates.
//!   Visible to every member at all times, also before the lock.
//! - `GET /api/me/picks` lists the caller's picks.
//!
//! Writes run in one `BEGIN IMMEDIATE` transaction: load the match, check
//! that it is pickable at the server's "now" (also before the lock ticker
//! ran), validate, save. Errors: `pick_locked` and `not_pickable` (409),
//! `invalid_pick` (422), `not_found` (404). See
//! `docs/adr/0010-pick-rules-and-api.md`.

use std::collections::HashMap;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, put};
use axum::{Json, Router};
use chrono::{DateTime, Utc};
use domain::picks::{self as rules, PickError, PickStats, Unpickable};
use domain::{MapPick, Match, MatchId, Pick, PickId, PickParts, ResultState, SeriesScore, TeamId};
use serde::{Deserialize, Serialize};
use sqlx::SqliteConnection;
use store::picks::PickResult;

use crate::AppState;
use crate::auth::Member;
use crate::error::{ApiError, JsonBody};
use crate::{img, me};

/// Routes to be merged under `/api`.
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/matches/{id}/pick", put(put_pick).delete(delete_pick))
        .route("/matches/{id}/picks", get(match_picks))
        .route("/me/picks", get(my_picks))
}

// ---- wire types -------------------------------------------------------------

/// A score relative to the match's team A/B.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Score<T> {
    pub a: T,
    pub b: T,
}

/// One map pick, in requests and responses.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MapPickDto {
    pub map_number: u8,
    pub winner_team_id: i64,
    /// Exact regulation score, 13:0 to 13:11 for either side.
    #[serde(default)]
    pub rounds: Option<Score<u16>>,
    /// The winner wins in overtime; excludes `rounds`.
    #[serde(default)]
    pub overtime: bool,
}

/// The three parts of a pick, in requests and responses. Every part is
/// optional; a PUT replaces the whole pick, so a part left out is removed.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PartsDto {
    #[serde(default)]
    pub winner_team_id: Option<i64>,
    #[serde(default)]
    pub series: Option<Score<u8>>,
    #[serde(default)]
    pub maps: Vec<MapPickDto>,
}

impl PartsDto {
    fn into_parts(self) -> PickParts {
        PickParts {
            winner_team_id: self.winner_team_id.map(TeamId),
            series: self.series.map(|s| SeriesScore { a: s.a, b: s.b }),
            maps: self
                .maps
                .into_iter()
                .map(|m| MapPick {
                    map_number: m.map_number,
                    winner_team_id: TeamId(m.winner_team_id),
                    rounds: m.rounds.map(|r| (r.a, r.b)),
                    overtime: m.overtime,
                })
                .collect(),
        }
    }

    fn from_parts(p: &PickParts) -> Self {
        Self {
            winner_team_id: p.winner_team_id.map(|t| t.0),
            series: p.series.map(|s| Score { a: s.a, b: s.b }),
            maps: p
                .maps
                .iter()
                .map(|m| MapPickDto {
                    map_number: m.map_number,
                    winner_team_id: m.winner_team_id.0,
                    rounds: m.rounds.map(|(a, b)| Score { a, b }),
                    overtime: m.overtime,
                })
                .collect(),
        }
    }
}

/// A pick of the caller: the PUT response and the entries of
/// `GET /api/me/picks`.
#[derive(Debug, Serialize)]
pub struct MyPickBody {
    pub match_id: i64,
    #[serde(flatten)]
    pub parts: PartsDto,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    /// Saved more than once.
    pub edited: bool,
}

impl MyPickBody {
    /// `revisions`: number of saves of the pick.
    fn new(p: &Pick, revisions: i64) -> Self {
        Self {
            match_id: p.match_id.0,
            parts: PartsDto::from_parts(&p.parts),
            created_at: p.created_at,
            updated_at: p.updated_at,
            edited: edited(revisions),
        }
    }
}

#[derive(Debug, Serialize)]
pub struct MyPicksBody {
    pub picks: Vec<MyPickBody>,
}

/// One member's pick as everyone sees it.
#[derive(Debug, Serialize)]
pub struct PickView {
    pub user_id: String,
    pub display_name: String,
    pub avatar_url: String,
    #[serde(flatten)]
    pub parts: PartsDto,
    pub created_at: DateTime<Utc>,
    /// Time of the last change.
    pub updated_at: DateTime<Utc>,
    pub edited: bool,
    /// The settled points (T11), once settlement ran for the match.
    pub result: Option<PickResultBody>,
}

/// The points of one pick, as settlement (T11) stored them.
#[derive(Debug, Serialize)]
pub struct PickResultBody {
    pub points: i64,
    pub state: ResultState,
    /// Parts still waiting for a result or for data.
    pub pending_parts: i64,
    /// `domain::scoring::Breakdown` as stored: every part's outcome and
    /// points.
    pub breakdown: serde_json::Value,
}

/// `GET /api/matches/{id}/picks`.
#[derive(Debug, Serialize)]
pub struct MatchPicksBody {
    pub match_id: i64,
    /// The caller may create, edit or delete their pick now.
    pub pickable: bool,
    /// Picks are read-only (the lock passed, or the match is live or over).
    pub locked: bool,
    pub lock_at: Option<DateTime<Utc>>,
    pub picks: Vec<PickView>,
    pub stats: PickStats,
}

/// A pick counts as edited once it was saved more than once. Counted from
/// the revisions, not the timestamps, which can be equal (frozen fixture
/// clock, two saves within a millisecond).
fn edited(revisions: i64) -> bool {
    revisions > 1
}

// ---- errors -----------------------------------------------------------------

impl From<Unpickable> for ApiError {
    fn from(u: Unpickable) -> Self {
        let code = match u {
            Unpickable::Locked => "pick_locked",
            Unpickable::NotPickable(_) => "not_pickable",
        };
        ApiError::new(StatusCode::CONFLICT, code, u.to_string())
    }
}

impl From<PickError> for ApiError {
    fn from(e: PickError) -> Self {
        ApiError::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "invalid_pick",
            e.to_string(),
        )
    }
}

fn db(e: sqlx::Error) -> ApiError {
    ApiError::db("picks", e)
}

fn match_not_found(id: MatchId) -> ApiError {
    ApiError::new(
        StatusCode::NOT_FOUND,
        "not_found",
        format!("match {id} not found"),
    )
}

// ---- handlers ---------------------------------------------------------------

/// Loads the match and checks that `member` may change their pick at `now`.
async fn pickable_match(
    conn: &mut SqliteConnection,
    member: &Member,
    id: MatchId,
    now: DateTime<Utc>,
) -> Result<Match, ApiError> {
    let m = store::matches::get(&mut *conn, id)
        .await
        .map_err(db)?
        .ok_or_else(|| match_not_found(id))?;
    let followed = store::guilds::is_followed(&mut *conn, &member.guild_id, m.fields.event_id)
        .await
        .map_err(db)?;
    rules::check_pickable(&m.fields, followed, now)?;
    Ok(m)
}

async fn put_pick(
    State(st): State<AppState>,
    member: Member,
    Path(id): Path<i64>,
    JsonBody(body): JsonBody<PartsDto>,
) -> Result<Response, ApiError> {
    let id = MatchId(id);
    let now = st.clock.now();
    let mut tx = store::begin_write(&st.pool).await.map_err(db)?;
    let m = pickable_match(&mut tx, &member, id, now).await?;
    let f = &m.fields;
    // A pickable match has both teams.
    let (Some(a), Some(b)) = (f.team_a_id, f.team_b_id) else {
        return Err(Unpickable::NotPickable(rules::NotPickableReason::TeamsUnknown).into());
    };
    match rules::validate(f.format, a, b, &body.into_parts()) {
        Err(PickError::Empty) => {
            // Removing every part deletes the pick.
            store::picks::delete(&mut *tx, &member.guild_id, &member.user_id, id)
                .await
                .map_err(db)?;
            tx.commit().await.map_err(db)?;
            Ok(StatusCode::NO_CONTENT.into_response())
        }
        Err(e) => Err(e.into()),
        Ok(parts) => {
            let pick =
                store::picks::save(&mut *tx, &member.guild_id, &member.user_id, id, &parts, now)
                    .await
                    .map_err(db)?;
            let revisions = store::picks::count_revisions(&mut *tx, pick.id)
                .await
                .map_err(db)?;
            tx.commit().await.map_err(db)?;
            Ok(Json(MyPickBody::new(&pick, revisions)).into_response())
        }
    }
}

async fn delete_pick(
    State(st): State<AppState>,
    member: Member,
    Path(id): Path<i64>,
) -> Result<StatusCode, ApiError> {
    let id = MatchId(id);
    let now = st.clock.now();
    let mut tx = store::begin_write(&st.pool).await.map_err(db)?;
    pickable_match(&mut tx, &member, id, now).await?;
    store::picks::delete(&mut *tx, &member.guild_id, &member.user_id, id)
        .await
        .map_err(db)?;
    tx.commit().await.map_err(db)?;
    Ok(StatusCode::NO_CONTENT)
}

async fn match_picks(
    State(st): State<AppState>,
    member: Member,
    Path(id): Path<i64>,
) -> Result<Json<MatchPicksBody>, ApiError> {
    let id = MatchId(id);
    let now = st.clock.now();
    // One read transaction, so match, picks and names are consistent.
    let mut tx = st.pool.begin().await.map_err(db)?;
    let m = store::matches::get(&mut *tx, id)
        .await
        .map_err(db)?
        .ok_or_else(|| match_not_found(id))?;
    let followed = store::guilds::is_followed(&mut *tx, &member.guild_id, m.fields.event_id)
        .await
        .map_err(db)?;
    let picks = store::picks::list_for_match_with_authors(&mut *tx, &member.guild_id, id)
        .await
        .map_err(db)?;
    let mut results: HashMap<PickId, PickResult> =
        store::picks::results_for_guild_match(&mut *tx, &member.guild_id, id)
            .await
            .map_err(db)?
            .into_iter()
            .map(|r| (r.pick_id, r))
            .collect();
    tx.commit().await.map_err(db)?;

    let f = &m.fields;
    let stats = rules::aggregate(
        f.format,
        f.team_a_id,
        f.team_b_id,
        picks.iter().map(|p| &p.pick.parts),
    );
    let picks = picks
        .into_iter()
        .map(|ap| {
            let a = &ap.author;
            PickView {
                display_name: me::display_name(
                    a.nick.as_deref(),
                    a.global_name.as_deref(),
                    &a.username,
                ),
                avatar_url: img::avatar_url(&ap.pick.user_id, a.avatar_hash.as_deref()),
                parts: PartsDto::from_parts(&ap.pick.parts),
                created_at: ap.pick.created_at,
                updated_at: ap.pick.updated_at,
                edited: edited(ap.revisions),
                result: results.remove(&ap.pick.id).map(|r| PickResultBody {
                    points: r.points,
                    state: r.state,
                    pending_parts: r.pending_parts,
                    breakdown: r.breakdown,
                }),
                user_id: ap.pick.user_id,
            }
        })
        .collect();
    Ok(Json(MatchPicksBody {
        match_id: id.0,
        pickable: rules::check_pickable(f, followed, now).is_ok(),
        locked: rules::is_locked(f, now),
        lock_at: f.lock_at,
        picks,
        stats,
    }))
}

async fn my_picks(
    State(st): State<AppState>,
    member: Member,
) -> Result<Json<MyPicksBody>, ApiError> {
    let picks = store::picks::list_for_user(&st.pool, &member.guild_id, &member.user_id)
        .await
        .map_err(db)?;
    Ok(Json(MyPicksBody {
        picks: picks.iter().map(|(p, n)| MyPickBody::new(p, *n)).collect(),
    }))
}

#[cfg(test)]
mod tests;
