//! The event view (T13).
//!
//! - `GET /api/events` lists the events the caller's guild follows: running
//!   first, then upcoming, then finished.
//! - `GET /api/events/{id}` is one event with its stages and every match;
//!   any member can open any event, like a match detail (ADR 0012).
//!
//! Each stage says how the Activity shows it (`view`): elimination stages
//! as a bracket tree with positions from `domain::bracket::positions`,
//! Swiss and group stages as standings with round numbers, anything else
//! as a plain list. See `docs/adr/0017-event-view-and-brackets.md`.

use std::cmp::Ordering;
use std::collections::HashMap;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::get;
use axum::{Json, Router};
use chrono::{DateTime, Utc};
use domain::bracket;
use domain::{
    BracketSide, EventId, EventStatus, EventTier, Feed, FeedOutcome, Match, StageId, StageKind,
    Team, TeamId,
};
use serde::Serialize;
use store::matches::MatchListItem;

use crate::AppState;
use crate::auth::Member;
use crate::error::ApiError;
use crate::matches::{MatchRow, RowContext, TeamRef};

/// Routes to be merged under `/api`.
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/events", get(list))
        .route("/events/{id}", get(detail))
}

// ---- wire types -------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
pub struct EventInfo {
    pub id: i64,
    pub name: String,
    pub tier: EventTier,
    pub status: EventStatus,
    pub begin_at: Option<DateTime<Utc>>,
    pub end_at: Option<DateTime<Utc>>,
}

impl From<&domain::Event> for EventInfo {
    fn from(e: &domain::Event) -> Self {
        Self {
            id: e.id.0,
            name: e.name.clone(),
            tier: e.tier,
            status: e.status,
            begin_at: e.begin_at,
            end_at: e.end_at,
        }
    }
}

/// One followed event in `GET /api/events`.
#[derive(Debug, Serialize)]
pub struct EventSummary {
    #[serde(flatten)]
    pub event: EventInfo,
    /// Matches of any state.
    pub matches: i64,
    /// Matches being played now.
    pub live: i64,
    /// Picks the guild made on the event.
    pub picks: i64,
}

/// `GET /api/events`.
#[derive(Debug, Serialize)]
pub struct EventList {
    pub events: Vec<EventSummary>,
}

/// How the Activity shows a stage.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StageViewKind {
    /// An elimination tree.
    Bracket,
    /// Standings and matches per round (Swiss, groups).
    Standings,
    /// Matches by start time.
    List,
}

#[derive(Debug, Clone, Serialize)]
pub struct FeedView {
    pub match_id: i64,
    pub outcome: FeedOutcome,
}

impl From<Feed> for FeedView {
    fn from(f: Feed) -> Self {
        Self {
            match_id: f.match_id.0,
            outcome: f.outcome,
        }
    }
}

/// Where a match sits in the tree.
#[derive(Debug, Clone, Serialize)]
pub struct BracketView {
    pub side: BracketSide,
    /// Column within the side, from 0.
    pub round: u32,
    /// Row within the column, from 0.
    pub slot: u32,
    /// Side, round or slot was derived from the feeds, not stored.
    pub derived: bool,
    /// Where team A comes from while it is TBD (or came from).
    pub feeds_a: Option<FeedView>,
    pub feeds_b: Option<FeedView>,
}

/// A match of a stage: the match list's row plus its place in the stage.
#[derive(Debug, Clone, Serialize)]
pub struct StageMatch {
    #[serde(flatten)]
    pub row: MatchRow,
    /// Set in a `bracket` stage.
    pub bracket: Option<BracketView>,
    /// The round (from 1) in a `standings` stage.
    pub round: Option<u32>,
}

/// One team's line in the standings.
#[derive(Debug, Clone, Serialize)]
pub struct StandingRow {
    pub team: TeamRef,
    pub wins: u32,
    pub losses: u32,
    pub map_diff: i32,
    pub round_diff: i32,
}

#[derive(Debug, Clone, Serialize)]
pub struct StageView {
    /// `None` for the matches without a stage.
    pub id: Option<i64>,
    pub name: String,
    pub kind: Option<StageKind>,
    pub view: StageViewKind,
    /// Set in a `standings` stage.
    pub standings: Option<Vec<StandingRow>>,
    /// By start time (unscheduled last), then ID.
    pub matches: Vec<StageMatch>,
}

/// `GET /api/events/{id}`.
#[derive(Debug, Serialize)]
pub struct EventDetail {
    pub event: EventInfo,
    /// The caller's guild follows the event: its matches are pickable.
    pub followed: bool,
    pub stages: Vec<StageView>,
}

// ---- helpers ------------------------------------------------------------------

fn db(e: sqlx::Error) -> ApiError {
    ApiError::db("events", e)
}

/// Running, then upcoming, then finished and canceled.
fn status_group(s: EventStatus) -> u8 {
    match s {
        EventStatus::Running => 0,
        EventStatus::Upcoming => 1,
        EventStatus::Finished | EventStatus::Canceled => 2,
    }
}

/// `None` last.
fn some_first<T: Ord>(a: Option<T>, b: Option<T>) -> Ordering {
    match (a, b) {
        (Some(a), Some(b)) => a.cmp(&b),
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => Ordering::Equal,
    }
}

/// The list order: by status group; running and upcoming events by start,
/// finished ones by end (else start), latest first; then by name and ID.
fn event_order(a: &domain::Event, b: &domain::Event) -> Ordering {
    let (ga, gb) = (status_group(a.status), status_group(b.status));
    ga.cmp(&gb)
        .then_with(|| {
            if ga == 2 {
                match (a.end_at.or(a.begin_at), b.end_at.or(b.begin_at)) {
                    (Some(x), Some(y)) => y.cmp(&x),
                    (x, y) => some_first(x, y),
                }
            } else {
                some_first(a.begin_at, b.begin_at)
            }
        })
        .then_with(|| a.name.cmp(&b.name))
        .then(a.id.cmp(&b.id))
}

/// Matches by start time (unscheduled last), then ID.
fn schedule_order(a: &MatchListItem, b: &MatchListItem) -> Ordering {
    some_first(a.m.fields.scheduled_at, b.m.fields.scheduled_at).then(a.m.id.cmp(&b.m.id))
}

fn view_kind(kind: Option<StageKind>, matches: &[Match]) -> StageViewKind {
    match kind {
        Some(StageKind::SingleElim | StageKind::DoubleElim) => StageViewKind::Bracket,
        Some(StageKind::Swiss | StageKind::Groups) => StageViewKind::Standings,
        _ if bracket::has_bracket_data(matches) => StageViewKind::Bracket,
        _ => StageViewKind::List,
    }
}

// ---- handlers -----------------------------------------------------------------

async fn list(State(st): State<AppState>, member: Member) -> Result<Json<EventList>, ApiError> {
    let mut rows = store::guilds::followed_overview(&st.pool, &member.guild_id)
        .await
        .map_err(db)?;
    rows.sort_by(|a, b| event_order(&a.event, &b.event));
    Ok(Json(EventList {
        events: rows
            .iter()
            .map(|r| EventSummary {
                event: EventInfo::from(&r.event),
                matches: r.matches,
                live: r.live,
                picks: r.picks,
            })
            .collect(),
    }))
}

async fn detail(
    State(st): State<AppState>,
    member: Member,
    Path(id): Path<i64>,
) -> Result<Json<EventDetail>, ApiError> {
    let id = EventId(id);
    let now = st.clock.now();
    // One read transaction, so stages, matches, maps and picks agree.
    let mut tx = st.pool.begin().await.map_err(db)?;
    let event = store::events::get(&mut *tx, id)
        .await
        .map_err(db)?
        .ok_or_else(|| {
            ApiError::new(
                StatusCode::NOT_FOUND,
                "not_found",
                format!("event {id} not found"),
            )
        })?;
    let followed = store::guilds::is_followed(&mut *tx, &member.guild_id, id)
        .await
        .map_err(db)?;
    let stages = store::events::list_stages(&mut *tx, id).await.map_err(db)?;
    let mut items = store::matches::list_event_view(&mut *tx, &member.guild_id, id)
        .await
        .map_err(db)?;
    let ctx = RowContext::load(&mut tx, &member, &items).await?;
    tx.commit().await.map_err(db)?;

    items.sort_by(schedule_order);
    let mut by_stage: HashMap<Option<StageId>, Vec<&MatchListItem>> = HashMap::new();
    for item in &items {
        by_stage
            .entry(item.m.fields.stage_id)
            .or_default()
            .push(item);
    }

    let mut views: Vec<StageView> = stages
        .iter()
        .map(|s| {
            let ms = by_stage.remove(&Some(s.id)).unwrap_or_default();
            stage_view(
                Some(s.id.0),
                s.name.clone(),
                Some(s.kind),
                &ms,
                &ctx,
                followed,
                now,
            )
        })
        .collect();
    // Matches without a stage, or of a stage of another event.
    let mut rest: Vec<&MatchListItem> = by_stage.into_values().flatten().collect();
    if !rest.is_empty() {
        rest.sort_by(|a, b| schedule_order(a, b));
        views.push(stage_view(
            None,
            "Matches".into(),
            None,
            &rest,
            &ctx,
            followed,
            now,
        ));
    }

    Ok(Json(EventDetail {
        event: EventInfo::from(&event),
        followed,
        stages: views,
    }))
}

fn stage_view(
    id: Option<i64>,
    name: String,
    kind: Option<StageKind>,
    items: &[&MatchListItem],
    ctx: &RowContext,
    followed: bool,
    now: DateTime<Utc>,
) -> StageView {
    let ms: Vec<Match> = items.iter().map(|i| i.m.clone()).collect();
    let view = view_kind(kind, &ms);
    let positions = (view == StageViewKind::Bracket).then(|| bracket::positions(&ms));
    let rounds = (view == StageViewKind::Standings).then(|| bracket::swiss_rounds(&ms));

    let standings = (view == StageViewKind::Standings).then(|| {
        let mut teams: HashMap<TeamId, &Team> = HashMap::new();
        for i in items {
            teams.extend(i.team_a.iter().chain(&i.team_b).map(|t| (t.id, t)));
        }
        let maps: Vec<domain::MatchMap> = ms
            .iter()
            .flat_map(|m| ctx.maps_of(m.id).iter().cloned())
            .collect();
        let by_name = |a: TeamId, b: TeamId| {
            let name = |t: TeamId| teams.get(&t).map(|t| t.name.to_lowercase());
            name(a).cmp(&name(b)).then(a.cmp(&b))
        };
        bracket::standings(&ms, &maps, by_name)
            .into_iter()
            .filter_map(|r| {
                Some(StandingRow {
                    team: TeamRef::new(teams.get(&r.team_id)?),
                    wins: r.wins,
                    losses: r.losses,
                    map_diff: r.map_diff,
                    round_diff: r.round_diff,
                })
            })
            .collect()
    });

    let matches = items
        .iter()
        .map(|item| {
            let b = &item.m.fields.bracket;
            StageMatch {
                row: ctx.row(item, followed, now),
                bracket: positions
                    .as_ref()
                    .and_then(|p| p.get(&item.m.id))
                    .map(|p| BracketView {
                        side: p.side,
                        round: p.round,
                        slot: p.slot,
                        derived: p.derived,
                        feeds_a: b.feeds_a_from.map(FeedView::from),
                        feeds_b: b.feeds_b_from.map(FeedView::from),
                    }),
                round: rounds.as_ref().and_then(|r| r.get(&item.m.id).copied()),
            }
        })
        .collect();

    StageView {
        id,
        name,
        kind,
        view,
        standings,
        matches,
    }
}

#[cfg(test)]
mod tests;
