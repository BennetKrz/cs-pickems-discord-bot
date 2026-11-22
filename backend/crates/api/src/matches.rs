//! The match list and match detail (T12).
//!
//! - `GET /api/matches?state=upcoming|live|finished&event_id=&cursor=` lists
//!   one segment of the matches of events the guild follows, with the
//!   caller's pick status and points.
//! - `GET /api/matches/{id}` is one match with its maps; any member can open
//!   any match, also after an unfollow (like `GET …/picks`).
//!
//! Segments (see `docs/adr/0012-matches-views.md`), all on the server clock:
//! - **upcoming**: `scheduled`/`postponed` starting within 14 days, earliest
//!   first; overdue matches stay until sync reports them live;
//! - **live**: earliest start first;
//! - **finished**: `finished`/`canceled` within the last 30 days, latest
//!   first, in pages with an opaque cursor.

use std::collections::HashMap;

use axum::extract::rejection::QueryRejection;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::get;
use axum::{Json, Router};
use chrono::{DateTime, Duration, Utc};
use domain::picks::{self as rules, Completeness};
use domain::{
    EventId, MapStatus, Match, MatchFormat, MatchId, MatchMap, MatchStatus, ResultState, Team,
    TeamId,
};
use serde::Serialize;
use store::matches::{ListQuery, MatchListItem, Segment};
use store::picks::PickResult;

use crate::AppState;
use crate::auth::Member;
use crate::error::ApiError;
use crate::img;

/// How far ahead the upcoming segment reaches.
pub const UPCOMING_DAYS: i64 = 14;
/// How far back the finished segment reaches.
pub const FINISHED_DAYS: i64 = 30;
/// Page size of the finished segment.
pub const FINISHED_PAGE: i64 = 30;
/// Page size of the upcoming and live segments. Their windows keep them
/// small; the cursor only guards against a flood.
pub const OPEN_PAGE: i64 = 200;

/// Routes to be merged under `/api`.
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/matches", get(list))
        .route("/matches/{id}", get(detail))
}

// ---- wire types -------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
pub struct Named {
    pub id: i64,
    pub name: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct TeamRef {
    pub id: i64,
    pub name: String,
    pub acronym: Option<String>,
    /// Relative URL through the image proxy; `None` without a logo.
    pub logo_url: Option<String>,
}

impl TeamRef {
    pub(crate) fn new(t: &Team) -> Self {
        Self {
            id: t.id.0,
            name: t.name.clone(),
            acronym: t.acronym.clone(),
            logo_url: img::team_logo_url(t.id.0, t.logo_url.as_deref()),
        }
    }
}

/// The map being played now.
#[derive(Debug, Clone, Serialize)]
pub struct CurrentMap {
    pub number: u8,
    pub map_name: Option<String>,
    pub rounds_a: Option<u16>,
    pub rounds_b: Option<u16>,
}

/// What the list row and the detail header share.
#[derive(Debug, Clone, Serialize)]
pub struct MatchHeader {
    pub id: i64,
    pub event: Named,
    pub stage: Option<Named>,
    /// `None` while TBD.
    pub team_a: Option<TeamRef>,
    pub team_b: Option<TeamRef>,
    pub format: MatchFormat,
    pub status: MatchStatus,
    /// An admin voided the match: no points for anyone.
    pub voided: bool,
    pub scheduled_at: Option<DateTime<Utc>>,
    pub started_at: Option<DateTime<Utc>>,
    pub ended_at: Option<DateTime<Utc>>,
    pub lock_at: Option<DateTime<Utc>>,
    /// Picks are read-only.
    pub locked: bool,
    /// The caller may create, edit or delete their pick now.
    pub pickable: bool,
    pub maps_won_a: u8,
    pub maps_won_b: u8,
    pub winner_team_id: Option<i64>,
    /// The live map, if one is being played.
    pub current_map: Option<CurrentMap>,
    /// Picks for the match in the caller's guild.
    pub pick_count: i64,
}

/// The caller's pick status in a row.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MyPickStatus {
    None,
    Partial,
    Full,
}

impl From<Completeness> for MyPickStatus {
    fn from(c: Completeness) -> Self {
        match c {
            Completeness::Partial => Self::Partial,
            Completeness::Full => Self::Full,
        }
    }
}

/// The caller's points for a match.
#[derive(Debug, Clone, Serialize)]
pub struct MyResult {
    pub points: i64,
    pub state: ResultState,
    pub pending_parts: i64,
}

impl From<&PickResult> for MyResult {
    fn from(r: &PickResult) -> Self {
        Self {
            points: r.points,
            state: r.state,
            pending_parts: r.pending_parts,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct MatchRow {
    #[serde(flatten)]
    pub header: MatchHeader,
    pub my_pick: MyPickStatus,
    pub my_result: Option<MyResult>,
}

/// `GET /api/matches`.
#[derive(Debug, Serialize)]
pub struct MatchList {
    pub state: &'static str,
    /// The guild follows at least one event; without, the list is empty
    /// and admins get a hint.
    pub following_any: bool,
    pub matches: Vec<MatchRow>,
    /// Pass as `cursor` for the next page; `None` on the last page.
    pub next_cursor: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct MapView {
    pub number: u8,
    /// `None` until the veto is known.
    pub map_name: Option<String>,
    pub status: MapStatus,
    pub rounds_a: Option<u16>,
    pub rounds_b: Option<u16>,
    pub winner_team_id: Option<i64>,
}

impl From<&MatchMap> for MapView {
    fn from(m: &MatchMap) -> Self {
        Self {
            number: m.number,
            map_name: m.map_name.clone(),
            status: m.status,
            rounds_a: m.rounds_a,
            rounds_b: m.rounds_b,
            winner_team_id: m.winner_team_id.map(|t| t.0),
        }
    }
}

/// `GET /api/matches/{id}`. The picks come from `GET …/picks`.
#[derive(Debug, Serialize)]
pub struct MatchDetail {
    #[serde(flatten)]
    pub header: MatchHeader,
    pub maps: Vec<MapView>,
}

// ---- helpers ------------------------------------------------------------------

fn db(e: sqlx::Error) -> ApiError {
    ApiError::db("matches", e)
}

fn invalid_query(message: impl Into<String>) -> ApiError {
    ApiError::new(StatusCode::UNPROCESSABLE_ENTITY, "invalid_query", message)
}

/// The cursor is `<sort time in ms>_<match id>`; clients treat it as opaque.
fn encode_cursor(at: DateTime<Utc>, id: MatchId) -> String {
    format!("{}_{}", at.timestamp_millis(), id.0)
}

fn decode_cursor(s: &str) -> Option<(DateTime<Utc>, MatchId)> {
    let (ms, id) = s.split_once('_')?;
    let at = DateTime::from_timestamp_millis(ms.parse().ok()?)?;
    Some((at, MatchId(id.parse().ok()?)))
}

fn current_map(maps: &[MatchMap]) -> Option<CurrentMap> {
    maps.iter()
        .find(|m| m.status == MapStatus::Live)
        .map(|m| CurrentMap {
            number: m.number,
            map_name: m.map_name.clone(),
            rounds_a: m.rounds_a,
            rounds_b: m.rounds_b,
        })
}

struct HeaderParts<'a> {
    m: &'a Match,
    event: Named,
    stage: Option<Named>,
    team_a: Option<&'a Team>,
    team_b: Option<&'a Team>,
    maps: &'a [MatchMap],
    pick_count: i64,
    followed: bool,
    now: DateTime<Utc>,
}

fn header(p: HeaderParts<'_>) -> MatchHeader {
    let f = &p.m.fields;
    MatchHeader {
        id: p.m.id.0,
        event: p.event,
        stage: p.stage,
        team_a: p.team_a.map(TeamRef::new),
        team_b: p.team_b.map(TeamRef::new),
        format: f.format,
        status: f.status,
        voided: f.voided,
        scheduled_at: f.scheduled_at,
        started_at: f.started_at,
        ended_at: f.ended_at,
        lock_at: f.lock_at,
        locked: rules::is_locked(f, p.now),
        pickable: rules::check_pickable(f, p.followed, p.now).is_ok(),
        maps_won_a: f.maps_won_a,
        maps_won_b: f.maps_won_b,
        winner_team_id: f.winner_team_id.map(|t| t.0),
        current_map: current_map(p.maps),
        pick_count: p.pick_count,
    }
}

// ---- handlers -----------------------------------------------------------------

async fn list(
    State(st): State<AppState>,
    member: Member,
    query: Result<Query<HashMap<String, String>>, QueryRejection>,
) -> Result<Json<MatchList>, ApiError> {
    let Query(q) = query.map_err(|e| invalid_query(e.body_text()))?;
    let now = st.clock.now();
    let (state, segment, bound, page) = match q.get("state").map(String::as_str) {
        Some("upcoming") => (
            "upcoming",
            Segment::Upcoming,
            Some(now + Duration::days(UPCOMING_DAYS)),
            OPEN_PAGE,
        ),
        Some("live") => ("live", Segment::Live, None, OPEN_PAGE),
        Some("finished") => (
            "finished",
            Segment::Finished,
            Some(now - Duration::days(FINISHED_DAYS)),
            FINISHED_PAGE,
        ),
        _ => return Err(invalid_query("state must be upcoming, live or finished")),
    };
    let event_id = match q.get("event_id").filter(|s| !s.is_empty()) {
        Some(s) => Some(EventId(
            s.parse()
                .map_err(|_| invalid_query("event_id must be a number"))?,
        )),
        None => None,
    };
    let after = match q.get("cursor").filter(|s| !s.is_empty()) {
        Some(s) => Some(decode_cursor(s).ok_or_else(|| invalid_query("invalid cursor"))?),
        None => None,
    };

    // One read transaction, so rows, maps and picks are consistent.
    let mut tx = st.pool.begin().await.map_err(db)?;
    let following_any = !store::guilds::list_followed(&mut *tx, &member.guild_id)
        .await
        .map_err(db)?
        .is_empty();
    let mut items = store::matches::list_view(
        &mut *tx,
        &ListQuery {
            guild_id: &member.guild_id,
            segment,
            bound,
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
    let ctx = RowContext::load(&mut tx, &member, &items).await?;
    tx.commit().await.map_err(db)?;

    let matches = items
        .iter()
        // The list only has followed events.
        .map(|item| ctx.row(item, true, now))
        .collect();
    Ok(Json(MatchList {
        state,
        following_any,
        matches,
        next_cursor,
    }))
}

/// What rows need beyond their [`MatchListItem`]: the maps and the
/// caller's picks. Shared with the event view (T13).
pub(crate) struct RowContext {
    pub maps: HashMap<MatchId, Vec<MatchMap>>,
    pub mine: HashMap<MatchId, (domain::Pick, Option<PickResult>)>,
}

impl RowContext {
    pub(crate) async fn load(
        conn: &mut sqlx::SqliteConnection,
        member: &Member,
        items: &[MatchListItem],
    ) -> Result<Self, ApiError> {
        let ids: Vec<MatchId> = items.iter().map(|i| i.m.id).collect();
        let maps = store::matches::maps_for(&mut *conn, &ids)
            .await
            .map_err(db)?;
        let mine = store::picks::for_user_matches(conn, &member.guild_id, &member.user_id, &ids)
            .await
            .map_err(db)?;
        let mut by_match: HashMap<MatchId, Vec<MatchMap>> = HashMap::new();
        for m in maps {
            by_match.entry(m.match_id).or_default().push(m);
        }
        Ok(Self {
            maps: by_match,
            mine: mine
                .into_iter()
                .map(|(p, r)| (p.match_id, (p, r)))
                .collect(),
        })
    }

    /// The maps of one match, by number.
    pub(crate) fn maps_of(&self, id: MatchId) -> &[MatchMap] {
        self.maps.get(&id).map_or(&[], Vec::as_slice)
    }

    /// The row of `item`; `followed` says whether the guild follows its
    /// event (else it is not pickable).
    pub(crate) fn row(&self, item: &MatchListItem, followed: bool, now: DateTime<Utc>) -> MatchRow {
        row(item, &self.maps, &self.mine, followed, now)
    }
}

fn row(
    item: &MatchListItem,
    maps: &HashMap<MatchId, Vec<MatchMap>>,
    mine: &HashMap<MatchId, (domain::Pick, Option<PickResult>)>,
    followed: bool,
    now: DateTime<Utc>,
) -> MatchRow {
    let m = &item.m;
    let f = &m.fields;
    let header = header(HeaderParts {
        m,
        event: Named {
            id: f.event_id.0,
            name: item.event_name.clone(),
        },
        stage: f
            .stage_id
            .zip(item.stage_name.clone())
            .map(|(id, name)| Named { id: id.0, name }),
        team_a: item.team_a.as_ref(),
        team_b: item.team_b.as_ref(),
        maps: maps.get(&m.id).map_or(&[], Vec::as_slice),
        pick_count: item.pick_count,
        followed,
        now,
    });
    let (my_pick, my_result) = match mine.get(&m.id) {
        Some((p, r)) => (
            rules::completeness(f.format, &p.parts).into(),
            r.as_ref().map(MyResult::from),
        ),
        None => (MyPickStatus::None, None),
    };
    MatchRow {
        header,
        my_pick,
        my_result,
    }
}

fn not_found(id: MatchId) -> ApiError {
    ApiError::new(
        StatusCode::NOT_FOUND,
        "not_found",
        format!("match {id} not found"),
    )
}

async fn team(
    conn: &mut sqlx::SqliteConnection,
    id: Option<TeamId>,
) -> Result<Option<Team>, ApiError> {
    match id {
        Some(id) => store::teams::get(&mut *conn, id).await.map_err(db),
        None => Ok(None),
    }
}

async fn detail(
    State(st): State<AppState>,
    member: Member,
    Path(id): Path<i64>,
) -> Result<Json<MatchDetail>, ApiError> {
    let id = MatchId(id);
    let now = st.clock.now();
    let mut tx = st.pool.begin().await.map_err(db)?;
    let m = store::matches::get(&mut *tx, id)
        .await
        .map_err(db)?
        .ok_or_else(|| not_found(id))?;
    let f = &m.fields;
    let event = store::events::get(&mut *tx, f.event_id)
        .await
        .map_err(db)?
        .ok_or_else(|| not_found(id))?;
    let stage = match f.stage_id {
        Some(s) => store::events::get_stage(&mut *tx, s).await.map_err(db)?,
        None => None,
    };
    let team_a = team(&mut tx, f.team_a_id).await?;
    let team_b = team(&mut tx, f.team_b_id).await?;
    let maps = store::matches::list_maps(&mut *tx, id).await.map_err(db)?;
    let followed = store::guilds::is_followed(&mut *tx, &member.guild_id, f.event_id)
        .await
        .map_err(db)?;
    let pick_count = store::picks::count_for_match(&mut *tx, &member.guild_id, id)
        .await
        .map_err(db)?;
    tx.commit().await.map_err(db)?;

    let header = header(HeaderParts {
        m: &m,
        event: Named {
            id: event.id.0,
            name: event.name,
        },
        stage: stage.map(|s| Named {
            id: s.id.0,
            name: s.name,
        }),
        team_a: team_a.as_ref(),
        team_b: team_b.as_ref(),
        maps: &maps,
        pick_count,
        followed,
        now,
    });
    Ok(Json(MatchDetail {
        header,
        maps: maps.iter().map(MapView::from).collect(),
    }))
}

#[cfg(test)]
mod tests;
