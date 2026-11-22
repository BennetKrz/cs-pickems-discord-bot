//! The server leaderboard (T14).
//!
//! `GET /api/leaderboard?event_id=` ranks every member of the caller's
//! guild with at least one settled pick, all time or for one event. The
//! numbers come from `pick_results` only, through `store::leaderboard::load`,
//! the same query the bot's `/leaderboard` uses. Provisional points from
//! live matches count and are reported apart. See
//! `docs/adr/0016-leaderboard.md`.

use std::collections::HashMap;

use axum::extract::rejection::QueryRejection;
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::routing::get;
use axum::{Json, Router};
use chrono::NaiveDate;
use domain::EventId;
use serde::Serialize;

use crate::AppState;
use crate::auth::Member;
use crate::error::ApiError;
use crate::img;
use crate::matches::Named;

/// Routes to be merged under `/api`.
pub fn routes() -> Router<AppState> {
    Router::new().route("/leaderboard", get(leaderboard))
}

/// One member on the board.
#[derive(Debug, Serialize)]
pub struct Row {
    /// 1-based; equal members share a rank and the next one skips (1, 1, 3).
    pub rank: u32,
    /// The rank before the latest match day; `None` if the member had no
    /// standing then.
    pub previous_rank: Option<u32>,
    pub user_id: String,
    pub display_name: String,
    pub avatar_url: String,
    /// All points, provisional ones included.
    pub points: i64,
    /// The part of `points` from live matches.
    pub provisional_points: i64,
    /// Picks on live matches: their points can still move.
    pub provisional_picks: u32,
    /// Picks with a result that is not void.
    pub settled_picks: u32,
    pub winner_hits: u32,
    /// Winner parts that are decided (hit or miss).
    pub winner_decided: u32,
    /// `winner_hits / winner_decided`; `None` before any is decided.
    pub winner_accuracy: Option<f64>,
    /// Exact series scores.
    pub series_hits: u32,
    /// Exact map scores (or correct overtime winners).
    pub map_hits: u32,
}

/// `GET /api/leaderboard`.
#[derive(Debug, Serialize)]
pub struct LeaderboardBody {
    /// The event filter; `None` for all time.
    pub event_id: Option<i64>,
    /// The events the guild follows, for the filter.
    pub events: Vec<Named>,
    /// The latest local day (guild time zone) with a settled pick.
    pub match_day: Option<NaiveDate>,
    /// Someone had a standing before `match_day`, so `previous_rank` means
    /// something; without, the UI shows no rank changes.
    pub has_previous: bool,
    /// Some results are provisional (a match is live): worth polling faster.
    pub live: bool,
    pub rows: Vec<Row>,
}

fn db(e: sqlx::Error) -> ApiError {
    ApiError::db("leaderboard", e)
}

async fn leaderboard(
    State(st): State<AppState>,
    member: Member,
    query: Result<Query<HashMap<String, String>>, QueryRejection>,
) -> Result<Json<LeaderboardBody>, ApiError> {
    let Query(q) = query.map_err(|e| {
        ApiError::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "invalid_query",
            e.body_text(),
        )
    })?;
    let event_id = match q.get("event_id").filter(|s| !s.is_empty()) {
        Some(s) => Some(EventId(s.parse().map_err(|_| {
            ApiError::new(
                StatusCode::UNPROCESSABLE_ENTITY,
                "invalid_query",
                "event_id must be a number",
            )
        })?)),
        None => None,
    };

    // One read transaction, so the board and the filter list agree.
    let mut tx = st.pool.begin().await.map_err(db)?;
    let events = store::leaderboard::filter_events(&mut *tx, &member.guild_id)
        .await
        .map_err(db)?;
    let lb = store::leaderboard::load(&mut *tx, &member.guild_id, event_id)
        .await
        .map_err(db)?;
    tx.commit().await.map_err(db)?;

    let rows: Vec<Row> = lb
        .board
        .rows
        .iter()
        .map(|r| {
            let s = &r.standing;
            let avatar = lb
                .members
                .get(&s.user_id)
                .and_then(|m| m.avatar_hash.as_deref());
            Row {
                rank: r.rank,
                previous_rank: r.previous_rank,
                display_name: lb.name_of(&s.user_id).to_owned(),
                avatar_url: img::avatar_url(&s.user_id, avatar),
                points: s.points,
                provisional_points: s.provisional_points,
                provisional_picks: s.provisional_picks,
                settled_picks: s.settled_picks,
                winner_hits: s.winner_hits,
                winner_decided: s.winner_decided,
                winner_accuracy: s.winner_accuracy(),
                series_hits: s.series_hits,
                map_hits: s.map_hits,
                user_id: s.user_id.clone(),
            }
        })
        .collect();
    Ok(Json(LeaderboardBody {
        event_id: event_id.map(|e| e.0),
        events: events
            .into_iter()
            .map(|e| Named {
                id: e.id.0,
                name: e.name,
            })
            .collect(),
        match_day: lb.board.match_day,
        has_previous: lb.board.has_previous,
        live: rows.iter().any(|r| r.provisional_picks > 0),
        rows,
    }))
}

#[cfg(test)]
mod tests;
