//! Member profiles and pick history (T15).
//!
//! - `GET /api/users/{id}/profile?event_id=`: rank, points, pick counts and
//!   accuracy per part of one member of the caller's guild. The standing
//!   is the member's row of `store::leaderboard::load` for the same filter,
//!   so it equals the leaderboard's numbers.
//! - `GET /api/users/{id}/picks?state=open|live|settled&event_id=&cursor=`:
//!   the matches the member picked, one segment, with their pick and its
//!   result. Settled comes in pages of [`SETTLED_PAGE`], latest first.
//!
//! Picks are public (vision: pick visibility), so any member can open any
//! other member's profile in the same guild. A user who neither signed in
//! to the guild nor picked there is a 404. See
//! `docs/adr/0018-player-profile.md`.

use std::collections::{HashMap, HashSet};

use axum::extract::rejection::QueryRejection;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::get;
use axum::{Json, Router};
use chrono::{DateTime, Utc};
use domain::leaderboard::{Standing, rate};
use domain::{EventId, MatchId, MatchMap};
use serde::Serialize;
use sqlx::SqliteConnection;
use store::profile::{PickQuery, PickSegment};

use crate::AppState;
use crate::auth::Member;
use crate::error::ApiError;
use crate::img;
use crate::matches::{MatchHeader, Named, decode_cursor, encode_cursor, item_header};
use crate::picks::{PartsDto, PickResultBody};

/// Page size of the settled segment.
pub const SETTLED_PAGE: i64 = 30;
/// Page size of the open and live segments; they are small by nature, the
/// cursor only guards against a flood.
pub const OPEN_PAGE: i64 = 200;

/// Routes to be merged under `/api`.
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/users/{id}/profile", get(profile))
        .route("/users/{id}/picks", get(picks))
}

// ---- wire types -------------------------------------------------------------

/// Hits among the decided (hit or miss) parts of one kind.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct Accuracy {
    pub hits: u32,
    pub decided: u32,
    /// `hits / decided`; `None` before any part is decided.
    pub rate: Option<f64>,
}

impl Accuracy {
    fn new(hits: u32, decided: u32) -> Self {
        Self {
            hits,
            decided,
            rate: rate(hits, decided),
        }
    }
}

/// Accuracy per part. Pending and void parts, and void results, don't
/// count.
#[derive(Debug, Clone, Serialize)]
pub struct AccuracyBody {
    pub winner: Accuracy,
    /// Exact series scores.
    pub series: Accuracy,
    pub map_winner: Accuracy,
    /// Exact map scores (or correct overtime winners).
    pub map_score: Accuracy,
}

impl From<&Standing> for AccuracyBody {
    fn from(s: &Standing) -> Self {
        Self {
            winner: Accuracy::new(s.winner_hits, s.winner_decided),
            series: Accuracy::new(s.series_hits, s.series_decided),
            map_winner: Accuracy::new(s.map_winner_hits, s.map_winner_decided),
            map_score: Accuracy::new(s.map_hits, s.map_score_decided),
        }
    }
}

/// The member's picks per segment, for the filter.
#[derive(Debug, Clone, Serialize)]
pub struct PickCountsBody {
    pub open: i64,
    pub live: i64,
    pub settled: i64,
    pub total: i64,
}

/// `GET /api/users/{id}/profile`.
#[derive(Debug, Serialize)]
pub struct ProfileBody {
    pub user_id: String,
    pub display_name: String,
    pub avatar_url: String,
    /// The profile is the caller's own.
    pub is_me: bool,
    /// The event filter; `None` for all time.
    pub event_id: Option<i64>,
    /// The events the guild follows, for the filter.
    pub events: Vec<Named>,
    /// The leaderboard rank; `None` without a settled (non-void) pick.
    pub rank: Option<u32>,
    /// The rank before the latest match day, as on the leaderboard.
    pub previous_rank: Option<u32>,
    /// All points, provisional ones included.
    pub points: i64,
    /// The part of `points` from live matches.
    pub provisional_points: i64,
    pub provisional_picks: u32,
    /// Picks with a result that is not void (the leaderboard's number).
    pub settled_picks: u32,
    pub accuracy: AccuracyBody,
    pub picks: PickCountsBody,
    /// A picked match is live: points can move, worth polling faster.
    pub live: bool,
}

/// One picked match in the history.
#[derive(Debug, Serialize)]
pub struct ProfilePick {
    #[serde(rename = "match")]
    pub header: MatchHeader,
    pub pick: PartsDto,
    pub updated_at: DateTime<Utc>,
    /// The settled points, once settlement ran for the match.
    pub result: Option<PickResultBody>,
}

/// `GET /api/users/{id}/picks`.
#[derive(Debug, Serialize)]
pub struct ProfilePicksBody {
    pub state: &'static str,
    pub picks: Vec<ProfilePick>,
    /// Pass as `cursor` for the next page; `None` on the last page.
    pub next_cursor: Option<String>,
}

// ---- helpers ----------------------------------------------------------------

fn db(e: sqlx::Error) -> ApiError {
    ApiError::db("profile", e)
}

fn invalid_query(message: impl Into<String>) -> ApiError {
    ApiError::new(StatusCode::UNPROCESSABLE_ENTITY, "invalid_query", message)
}

fn parse_query(
    query: Result<Query<HashMap<String, String>>, QueryRejection>,
) -> Result<HashMap<String, String>, ApiError> {
    let Query(q) = query.map_err(|e| invalid_query(e.body_text()))?;
    Ok(q)
}

fn event_of(q: &HashMap<String, String>) -> Result<Option<EventId>, ApiError> {
    match q.get("event_id").filter(|s| !s.is_empty()) {
        Some(s) => {
            Ok(Some(EventId(s.parse().map_err(|_| {
                invalid_query("event_id must be a number")
            })?)))
        }
        None => Ok(None),
    }
}

/// 404 unless `user_id` belongs to the caller's guild.
async fn ensure_member(
    conn: &mut SqliteConnection,
    member: &Member,
    user_id: &str,
) -> Result<(), ApiError> {
    if user_id == member.user_id
        || store::profile::is_member(&mut *conn, &member.guild_id, user_id)
            .await
            .map_err(db)?
    {
        Ok(())
    } else {
        Err(ApiError::new(
            StatusCode::NOT_FOUND,
            "not_found",
            format!("member {user_id} not found"),
        ))
    }
}

// ---- handlers ---------------------------------------------------------------

async fn profile(
    State(st): State<AppState>,
    member: Member,
    Path(user_id): Path<String>,
    query: Result<Query<HashMap<String, String>>, QueryRejection>,
) -> Result<Json<ProfileBody>, ApiError> {
    let q = parse_query(query)?;
    let event_id = event_of(&q)?;

    // One read transaction, so the standing, the counts and the filter list
    // agree.
    let mut tx = st.pool.begin().await.map_err(db)?;
    ensure_member(&mut tx, &member, &user_id).await?;
    let events = store::leaderboard::filter_events(&mut *tx, &member.guild_id)
        .await
        .map_err(db)?;
    let lb = store::leaderboard::load(&mut *tx, &member.guild_id, event_id)
        .await
        .map_err(db)?;
    let counts = store::profile::pick_counts(&mut *tx, &member.guild_id, &user_id, event_id)
        .await
        .map_err(db)?;
    // The member may not be on the board (no settled pick yet).
    let info = match lb.members.get(&user_id) {
        Some(m) => Some(m.clone()),
        None => {
            store::leaderboard::members(&mut *tx, &member.guild_id, std::slice::from_ref(&user_id))
                .await
                .map_err(db)?
                .pop()
        }
    };
    tx.commit().await.map_err(db)?;

    let row = lb.board.rows.iter().find(|r| r.standing.user_id == user_id);
    let empty = Standing::new(&user_id);
    let s = row.map_or(&empty, |r| &r.standing);
    Ok(Json(ProfileBody {
        display_name: info
            .as_ref()
            .map_or_else(|| user_id.clone(), |m| m.display_name.clone()),
        avatar_url: img::avatar_url(
            &user_id,
            info.as_ref().and_then(|m| m.avatar_hash.as_deref()),
        ),
        is_me: user_id == member.user_id,
        event_id: event_id.map(|e| e.0),
        events: events
            .into_iter()
            .map(|e| Named {
                id: e.id.0,
                name: e.name,
            })
            .collect(),
        rank: row.map(|r| r.rank),
        previous_rank: row.and_then(|r| r.previous_rank),
        points: s.points,
        provisional_points: s.provisional_points,
        provisional_picks: s.provisional_picks,
        settled_picks: s.settled_picks,
        accuracy: AccuracyBody::from(s),
        picks: PickCountsBody {
            open: counts.open,
            live: counts.live,
            settled: counts.settled,
            total: counts.total(),
        },
        live: counts.live > 0 || s.provisional_picks > 0,
        user_id,
    }))
}

async fn picks(
    State(st): State<AppState>,
    member: Member,
    Path(user_id): Path<String>,
    query: Result<Query<HashMap<String, String>>, QueryRejection>,
) -> Result<Json<ProfilePicksBody>, ApiError> {
    let q = parse_query(query)?;
    let (state, segment, page) = match q.get("state").map(String::as_str) {
        Some("open") => ("open", PickSegment::Open, OPEN_PAGE),
        Some("live") => ("live", PickSegment::Live, OPEN_PAGE),
        Some("settled") => ("settled", PickSegment::Settled, SETTLED_PAGE),
        _ => return Err(invalid_query("state must be open, live or settled")),
    };
    let event_id = event_of(&q)?;
    let after = match q.get("cursor").filter(|s| !s.is_empty()) {
        Some(s) => Some(decode_cursor(s).ok_or_else(|| invalid_query("invalid cursor"))?),
        None => None,
    };
    let now = st.clock.now();

    // One read transaction, so matches, maps, picks and results agree.
    let mut tx = st.pool.begin().await.map_err(db)?;
    ensure_member(&mut tx, &member, &user_id).await?;
    let mut items = store::profile::list_picks(
        &mut *tx,
        &PickQuery {
            guild_id: &member.guild_id,
            user_id: &user_id,
            segment,
            event_id,
            after,
            limit: page + 1,
        },
    )
    .await
    .map_err(db)?;
    let next_cursor = if items.len() > usize::try_from(page).unwrap_or(usize::MAX) {
        items.truncate(usize::try_from(page).unwrap_or(0));
        items
            .last()
            .and_then(|i| i.sort_at.map(|at| encode_cursor(at, i.m.id)))
    } else {
        None
    };
    let ids: Vec<MatchId> = items.iter().map(|i| i.m.id).collect();
    let followed: HashSet<EventId> = store::guilds::list_followed(&mut *tx, &member.guild_id)
        .await
        .map_err(db)?
        .into_iter()
        .map(|f| f.event_id)
        .collect();
    let mut maps: HashMap<MatchId, Vec<MatchMap>> = HashMap::new();
    for m in store::matches::maps_for(&mut *tx, &ids).await.map_err(db)? {
        maps.entry(m.match_id).or_default().push(m);
    }
    let mut theirs: HashMap<MatchId, _> =
        store::picks::for_user_matches(&mut tx, &member.guild_id, &user_id, &ids)
            .await
            .map_err(db)?
            .into_iter()
            .map(|(p, r)| (p.match_id, (p, r)))
            .collect();
    tx.commit().await.map_err(db)?;

    let picks = items
        .iter()
        .filter_map(|item| {
            // Listed because the member picked it, so the pick is there.
            let (pick, result) = theirs.remove(&item.m.id)?;
            let header = item_header(
                item,
                maps.get(&item.m.id).map_or(&[], Vec::as_slice),
                followed.contains(&item.m.fields.event_id),
                now,
            );
            Some(ProfilePick {
                header,
                pick: PartsDto::from_parts(&pick.parts),
                updated_at: pick.updated_at,
                result: result.map(|r| PickResultBody {
                    points: r.points,
                    state: r.state,
                    pending_parts: r.pending_parts,
                    breakdown: r.breakdown,
                }),
            })
        })
        .collect();
    Ok(Json(ProfilePicksBody {
        state,
        picks,
        next_cursor,
    }))
}

#[cfg(test)]
mod tests;
