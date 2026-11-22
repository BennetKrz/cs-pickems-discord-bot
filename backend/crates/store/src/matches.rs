//! Matches and their maps.

use chrono::{DateTime, Utc};
use domain::{
    BracketPos, EventId, Feed, Match, MatchFields, MatchId, MatchMap, NewMatch, StageId, Team,
    TeamId,
};
use sqlx::sqlite::SqliteRow;
use sqlx::{Row, SqliteExecutor};

use crate::row::{enum_col, opt_enum_col};
use crate::{DbResult, Ts};

/// Every column but `id`, in the order [`bind_fields!`] binds them.
macro_rules! field_columns {
    () => {
        "event_id, stage_id, team_a_id, team_b_id, format, status, \
    scheduled_at, started_at, ended_at, lock_at, locked, voided, maps_won_a, maps_won_b, \
    winner_team_id, bracket_side, round_index, slot_index, \
    feeds_a_from_match_id, feeds_a_from_outcome, feeds_b_from_match_id, feeds_b_from_outcome"
    };
}

macro_rules! map_columns {
    () => {
        "match_id, number, map_name, status, rounds_a, rounds_b, winner_team_id"
    };
}

/// Binds all [`MatchFields`] in `field_columns!` order.
macro_rules! bind_fields {
    ($query:expr, $f:expr) => {{
        let f: &MatchFields = $f;
        let b = &f.bracket;
        $query
            .bind(f.event_id.0)
            .bind(f.stage_id.map(|id| id.0))
            .bind(f.team_a_id.map(|id| id.0))
            .bind(f.team_b_id.map(|id| id.0))
            .bind(f.format.as_str())
            .bind(f.status.as_str())
            .bind(Ts::opt(f.scheduled_at))
            .bind(Ts::opt(f.started_at))
            .bind(Ts::opt(f.ended_at))
            .bind(Ts::opt(f.lock_at))
            .bind(f.locked)
            .bind(f.voided)
            .bind(f.maps_won_a)
            .bind(f.maps_won_b)
            .bind(f.winner_team_id.map(|id| id.0))
            .bind(b.side.map(|s| s.as_str()))
            .bind(b.round_index)
            .bind(b.slot_index)
            .bind(b.feeds_a_from.map(|feed| feed.match_id.0))
            .bind(b.feeds_a_from.map(|feed| feed.outcome.as_str()))
            .bind(b.feeds_b_from.map(|feed| feed.match_id.0))
            .bind(b.feeds_b_from.map(|feed| feed.outcome.as_str()))
    }};
}

fn feed_from_row(row: &SqliteRow, id_col: &str, outcome_col: &str) -> DbResult<Option<Feed>> {
    let id: Option<i64> = row.try_get(id_col)?;
    let outcome = opt_enum_col(row, outcome_col)?;
    // The schema guarantees both are set or both are NULL.
    Ok(id.zip(outcome).map(|(id, outcome)| Feed {
        match_id: MatchId(id),
        outcome,
    }))
}

fn from_row(row: SqliteRow) -> DbResult<Match> {
    let team = |col: &str| -> DbResult<Option<TeamId>> {
        Ok(row.try_get::<Option<i64>, _>(col)?.map(TeamId))
    };
    Ok(Match {
        id: MatchId(row.try_get("id")?),
        fields: MatchFields {
            event_id: EventId(row.try_get("event_id")?),
            stage_id: row.try_get::<Option<i64>, _>("stage_id")?.map(StageId),
            team_a_id: team("team_a_id")?,
            team_b_id: team("team_b_id")?,
            format: enum_col(&row, "format")?,
            status: enum_col(&row, "status")?,
            scheduled_at: row.try_get("scheduled_at")?,
            started_at: row.try_get("started_at")?,
            ended_at: row.try_get("ended_at")?,
            lock_at: row.try_get("lock_at")?,
            locked: row.try_get("locked")?,
            voided: row.try_get("voided")?,
            maps_won_a: row.try_get("maps_won_a")?,
            maps_won_b: row.try_get("maps_won_b")?,
            winner_team_id: team("winner_team_id")?,
            bracket: BracketPos {
                side: opt_enum_col(&row, "bracket_side")?,
                round_index: row.try_get("round_index")?,
                slot_index: row.try_get("slot_index")?,
                feeds_a_from: feed_from_row(&row, "feeds_a_from_match_id", "feeds_a_from_outcome")?,
                feeds_b_from: feed_from_row(&row, "feeds_b_from_match_id", "feeds_b_from_outcome")?,
            },
        },
    })
}

fn map_from_row(row: SqliteRow) -> DbResult<MatchMap> {
    Ok(MatchMap {
        match_id: MatchId(row.try_get("match_id")?),
        number: row.try_get("number")?,
        map_name: row.try_get("map_name")?,
        status: enum_col(&row, "status")?,
        rounds_a: row.try_get("rounds_a")?,
        rounds_b: row.try_get("rounds_b")?,
        winner_team_id: row.try_get::<Option<i64>, _>("winner_team_id")?.map(TeamId),
    })
}

pub async fn insert(ex: impl SqliteExecutor<'_>, m: &NewMatch) -> DbResult<MatchId> {
    let sql = concat!(
        "INSERT INTO matches (",
        field_columns!(),
        ") VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?) RETURNING id"
    );
    let id: i64 = bind_fields!(sqlx::query_scalar(sql), m)
        .fetch_one(ex)
        .await?;
    Ok(MatchId(id))
}

/// Overwrites every field of an existing match. Returns `false` if it does
/// not exist. Team A/B orientation is the caller's responsibility: never swap
/// the teams of an existing match (see the key invariants).
pub async fn update(ex: impl SqliteExecutor<'_>, m: &Match) -> DbResult<bool> {
    // Same order as `field_columns!`.
    let sql = "UPDATE matches SET event_id = ?, stage_id = ?, team_a_id = ?, team_b_id = ?, \
        format = ?, status = ?, scheduled_at = ?, started_at = ?, ended_at = ?, lock_at = ?, \
        locked = ?, voided = ?, maps_won_a = ?, maps_won_b = ?, winner_team_id = ?, bracket_side = ?, \
        round_index = ?, slot_index = ?, feeds_a_from_match_id = ?, feeds_a_from_outcome = ?, \
        feeds_b_from_match_id = ?, feeds_b_from_outcome = ? WHERE id = ?";
    let res = bind_fields!(sqlx::query(sql), &m.fields)
        .bind(m.id.0)
        .execute(ex)
        .await?;
    Ok(res.rows_affected() == 1)
}

/// Locks the picks of every unlocked match whose `lock_at` has passed or
/// that is already live or finished (T10 lock rule, run by the lock ticker
/// in T06). Returns the IDs it locked; running it again locks nothing new.
pub async fn lock_due(ex: impl SqliteExecutor<'_>, now: DateTime<Utc>) -> DbResult<Vec<MatchId>> {
    sqlx::query_scalar(
        "UPDATE matches SET locked = 1 \
         WHERE locked = 0 AND (lock_at <= ? OR status IN ('live', 'finished')) \
         RETURNING id",
    )
    .bind(Ts(now))
    .fetch_all(ex)
    .await
    .map(|ids: Vec<i64>| {
        let mut ids: Vec<MatchId> = ids.into_iter().map(MatchId).collect();
        ids.sort_by_key(|id| id.0);
        ids
    })
}

pub async fn get(ex: impl SqliteExecutor<'_>, id: MatchId) -> DbResult<Option<Match>> {
    sqlx::query(concat!(
        "SELECT id, ",
        field_columns!(),
        " FROM matches WHERE id = ?"
    ))
    .bind(id.0)
    .try_map(from_row)
    .fetch_optional(ex)
    .await
}

/// The matches of an event by scheduled time; unscheduled ones last.
pub async fn list_for_event(
    ex: impl SqliteExecutor<'_>,
    event_id: EventId,
) -> DbResult<Vec<Match>> {
    sqlx::query(concat!(
        "SELECT id, ",
        field_columns!(),
        " FROM matches WHERE event_id = ? ORDER BY scheduled_at IS NULL, scheduled_at, id"
    ))
    .bind(event_id.0)
    .try_map(from_row)
    .fetch_all(ex)
    .await
}

/// Matches of an event between two teams (in either order) scheduled within
/// `[from, to]`, closest to the middle of the window first. For cross-source
/// identity (T03).
pub async fn find_between(
    ex: impl SqliteExecutor<'_>,
    event_id: EventId,
    team1: TeamId,
    team2: TeamId,
    from: DateTime<Utc>,
    to: DateTime<Utc>,
) -> DbResult<Vec<Match>> {
    let mid = from + (to - from) / 2;
    sqlx::query(concat!(
        "SELECT id, ",
        field_columns!(),
        " FROM matches WHERE event_id = ?1 \
           AND ((team_a_id = ?2 AND team_b_id = ?3) OR (team_a_id = ?3 AND team_b_id = ?2)) \
           AND scheduled_at BETWEEN ?4 AND ?5 \
         ORDER BY abs(julianday(scheduled_at) - julianday(?6)), id"
    ))
    .bind(event_id.0)
    .bind(team1.0)
    .bind(team2.0)
    .bind(Ts(from))
    .bind(Ts(to))
    .bind(Ts(mid))
    .try_map(from_row)
    .fetch_all(ex)
    .await
}

/// Inserts a map, or overwrites the map with the same match and number.
pub async fn upsert_map(ex: impl SqliteExecutor<'_>, map: &MatchMap) -> DbResult<()> {
    sqlx::query(concat!(
        "INSERT INTO maps (",
        map_columns!(),
        ") VALUES (?, ?, ?, ?, ?, ?, ?) \
         ON CONFLICT (match_id, number) DO UPDATE SET \
           map_name = excluded.map_name, status = excluded.status, \
           rounds_a = excluded.rounds_a, rounds_b = excluded.rounds_b, \
           winner_team_id = excluded.winner_team_id"
    ))
    .bind(map.match_id.0)
    .bind(map.number)
    .bind(&map.map_name)
    .bind(map.status.as_str())
    .bind(map.rounds_a)
    .bind(map.rounds_b)
    .bind(map.winner_team_id.map(|id| id.0))
    .execute(ex)
    .await?;
    Ok(())
}

/// The maps of a match, by number.
pub async fn list_maps(ex: impl SqliteExecutor<'_>, match_id: MatchId) -> DbResult<Vec<MatchMap>> {
    sqlx::query(concat!(
        "SELECT ",
        map_columns!(),
        " FROM maps WHERE match_id = ? ORDER BY number"
    ))
    .bind(match_id.0)
    .try_map(map_from_row)
    .fetch_all(ex)
    .await
}

// ---- match list for the Activity (T12) -----------------------------------------

/// A segment of the match list (T12). Each has its own statuses, sort key
/// and direction; see `docs/adr/0012-matches-views.md`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Segment {
    /// `scheduled` or `postponed` with a start time up to `bound`; by start
    /// time, ascending. Overdue matches that are not live yet stay here.
    Upcoming,
    /// `live`, by start, ascending. `bound` is ignored.
    Live,
    /// `finished` or `canceled` that ended (or started, or were scheduled)
    /// at or after `bound`; latest first.
    Finished,
}

/// What [`list_view`] lists.
#[derive(Debug, Clone)]
pub struct ListQuery<'a> {
    /// Only matches of events this guild follows; pick counts are for it.
    pub guild_id: &'a str,
    pub segment: Segment,
    /// Upper bound of the start (upcoming) or lower bound of the end
    /// (finished).
    pub bound: Option<DateTime<Utc>>,
    pub event_id: Option<EventId>,
    /// Keyset cursor: list only matches after this `(sort_at, id)`.
    pub after: Option<(DateTime<Utc>, MatchId)>,
    pub limit: i64,
}

/// One match of the list with what a row shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MatchListItem {
    pub m: Match,
    pub event_name: String,
    pub stage_name: Option<String>,
    /// `None` while TBD.
    pub team_a: Option<Team>,
    pub team_b: Option<Team>,
    /// Picks for the match in the guild.
    pub pick_count: i64,
    /// The segment's sort key; with `m.id` it forms the paging cursor.
    pub sort_at: Option<DateTime<Utc>>,
}

/// The list query of one segment. `$key` is the sort key, `$where` the
/// segment's condition (on `m` and `?3`, the bound), `$cmp`/`$dir` the
/// cursor comparison and order. Parameters: `?1` guild, `?2` event or NULL,
/// `?3` bound, `?4`/`?5` cursor time and ID (NULL for the first page),
/// `?6` limit. Only matches of events the guild follows, unless the
/// `@from` arm passes another join (`""` for none).
macro_rules! list_sql {
    ($key:literal, $where:literal, $cmp:literal, $dir:literal) => {
        list_sql!(
            @from "JOIN followed_events fe ON fe.event_id = m.event_id AND fe.guild_id = ?1 ",
            $key,
            $where,
            $cmp,
            $dir
        )
    };
    (@from $follow:literal, $key:literal, $where:literal, $cmp:literal, $dir:literal) => {
        concat!(
            "SELECT m.*, e.name AS event_name, s.name AS stage_name, \
               ta.name AS ta_name, ta.acronym AS ta_acronym, ta.logo_url AS ta_logo_url, \
               ta.country AS ta_country, \
               tb.name AS tb_name, tb.acronym AS tb_acronym, tb.logo_url AS tb_logo_url, \
               tb.country AS tb_country, \
               (SELECT COUNT(*) FROM picks p WHERE p.guild_id = ?1 AND p.match_id = m.id) \
                 AS pick_count \
             FROM (SELECT id, ",
            field_columns!(),
            ", ",
            $key,
            " AS sort_at FROM matches) m ",
            $follow,
            "JOIN events e ON e.id = m.event_id \
             LEFT JOIN stages s ON s.id = m.stage_id \
             LEFT JOIN teams ta ON ta.id = m.team_a_id \
             LEFT JOIN teams tb ON tb.id = m.team_b_id \
             WHERE ",
            $where,
            " AND (?2 IS NULL OR m.event_id = ?2) \
               AND (?4 IS NULL OR m.sort_at ",
            $cmp,
            " ?4 OR (m.sort_at = ?4 AND m.id ",
            $cmp,
            " ?5)) \
             ORDER BY m.sort_at ",
            $dir,
            ", m.id ",
            $dir,
            " LIMIT ?6"
        )
    };
}

const UPCOMING_SQL: &str = list_sql!(
    "scheduled_at",
    "m.status IN ('scheduled', 'postponed') AND m.sort_at IS NOT NULL \
     AND (?3 IS NULL OR m.sort_at <= ?3)",
    ">",
    "ASC"
);
const LIVE_SQL: &str = list_sql!(
    "coalesce(started_at, scheduled_at, '')",
    "m.status = 'live' AND (?3 IS NULL OR TRUE)",
    ">",
    "ASC"
);
const FINISHED_SQL: &str = list_sql!(
    "coalesce(ended_at, started_at, scheduled_at)",
    "m.status IN ('finished', 'canceled') AND m.sort_at IS NOT NULL \
     AND (?3 IS NULL OR m.sort_at >= ?3)",
    "<",
    "DESC"
);

fn team_from_prefixed(row: &SqliteRow, prefix: &str, id: Option<TeamId>) -> DbResult<Option<Team>> {
    let Some(id) = id else { return Ok(None) };
    let col = |c: &str| format!("{prefix}_{c}");
    Ok(Some(Team {
        id,
        name: row.try_get(col("name").as_str())?,
        acronym: row.try_get(col("acronym").as_str())?,
        logo_url: row.try_get(col("logo_url").as_str())?,
        country: row.try_get(col("country").as_str())?,
    }))
}

fn list_item_from_row(row: SqliteRow) -> DbResult<MatchListItem> {
    let event_name = row.try_get("event_name")?;
    let stage_name = row.try_get("stage_name")?;
    let pick_count = row.try_get("pick_count")?;
    // An empty string is the live key of a match without any time.
    let sort_at: Option<String> = row.try_get("sort_at")?;
    let sort_at = sort_at
        .filter(|s| !s.is_empty())
        .map(|s| {
            DateTime::parse_from_rfc3339(&s)
                .map(|t| t.with_timezone(&Utc))
                .map_err(|e| sqlx::Error::ColumnDecode {
                    index: "sort_at".to_owned(),
                    source: Box::new(e),
                })
        })
        .transpose()?;
    let team_a_id = row.try_get::<Option<i64>, _>("team_a_id")?.map(TeamId);
    let team_b_id = row.try_get::<Option<i64>, _>("team_b_id")?.map(TeamId);
    let team_a = team_from_prefixed(&row, "ta", team_a_id)?;
    let team_b = team_from_prefixed(&row, "tb", team_b_id)?;
    Ok(MatchListItem {
        m: from_row(row)?,
        event_name,
        stage_name,
        team_a,
        team_b,
        pick_count,
        sort_at,
    })
}

/// The matches of one segment of the list (T12), only of events the guild
/// follows, with event, stage, teams and the guild's pick count.
pub async fn list_view(
    ex: impl SqliteExecutor<'_>,
    q: &ListQuery<'_>,
) -> DbResult<Vec<MatchListItem>> {
    let sql = match q.segment {
        Segment::Upcoming => UPCOMING_SQL,
        Segment::Live => LIVE_SQL,
        Segment::Finished => FINISHED_SQL,
    };
    sqlx::query(sql)
        .bind(q.guild_id)
        .bind(q.event_id.map(|e| e.0))
        .bind(Ts::opt(q.bound))
        .bind(q.after.map(|(t, _)| Ts(t)))
        .bind(q.after.map(|(_, id)| id.0))
        .bind(q.limit)
        .try_map(list_item_from_row)
        .fetch_all(ex)
        .await
}

/// Matches whose lock time (`lock_at`, else `scheduled_at`) falls in
/// `[from, to)`, by that time. `?7` is the window's end; event filter and
/// cursor are unused.
const WINDOW_SQL: &str = list_sql!(
    "coalesce(lock_at, scheduled_at)",
    "m.status <> 'canceled' AND m.sort_at IS NOT NULL AND m.sort_at >= ?3 AND m.sort_at < ?7",
    ">",
    "ASC"
);

/// The daily digest's matches (T08): every match of an event the guild
/// follows whose lock time is in `[from, to)`, except canceled ones, by
/// lock time, with teams and the guild's pick count. At most `limit`.
pub async fn list_lock_window(
    ex: impl SqliteExecutor<'_>,
    guild_id: &str,
    from: DateTime<Utc>,
    to: DateTime<Utc>,
    limit: i64,
) -> DbResult<Vec<MatchListItem>> {
    sqlx::query(WINDOW_SQL)
        .bind(guild_id)
        .bind(None::<i64>)
        .bind(Ts(from))
        .bind(None::<Ts>)
        .bind(None::<i64>)
        .bind(limit)
        .bind(Ts(to))
        .try_map(list_item_from_row)
        .fetch_all(ex)
        .await
}

/// Several matches by ID, with teams and the guild's pick count, by start
/// time. Only matches of events the guild follows.
pub async fn list_by_ids(
    ex: impl SqliteExecutor<'_>,
    guild_id: &str,
    ids: &[MatchId],
) -> DbResult<Vec<MatchListItem>> {
    let ids: Vec<i64> = ids.iter().map(|m| m.0).collect();
    sqlx::query(BY_IDS_SQL)
        .bind(guild_id)
        .bind(None::<i64>)
        .bind(None::<Ts>)
        .bind(None::<Ts>)
        .bind(None::<i64>)
        .bind(ids.len() as i64)
        .bind(sqlx::types::Json(ids))
        .try_map(list_item_from_row)
        .fetch_all(ex)
        .await
}

/// Every match of event `?2`, followed or not, by start time (unscheduled
/// first).
const EVENT_SQL: &str = list_sql!(
    @from "",
    "scheduled_at",
    "m.event_id = ?2 AND (?3 IS NULL OR TRUE)",
    ">",
    "ASC"
);

/// The upper bound of matches [`list_event_view`] returns; far above any
/// real event.
pub const EVENT_VIEW_LIMIT: i64 = 2000;

/// Every match of an event in any state, followed by the guild or not, for
/// the event view (T13): with event, stage, teams and the guild's pick
/// count, by start time (unscheduled first), at most [`EVENT_VIEW_LIMIT`].
pub async fn list_event_view(
    ex: impl SqliteExecutor<'_>,
    guild_id: &str,
    event_id: EventId,
) -> DbResult<Vec<MatchListItem>> {
    sqlx::query(EVENT_SQL)
        .bind(guild_id)
        .bind(event_id.0)
        .bind(None::<Ts>)
        .bind(None::<Ts>)
        .bind(None::<i64>)
        .bind(EVENT_VIEW_LIMIT)
        .try_map(list_item_from_row)
        .fetch_all(ex)
        .await
}

/// Matches by ID (`?7`, a JSON array), by start time (unscheduled first).
const BY_IDS_SQL: &str = list_sql!(
    "scheduled_at",
    "m.id IN (SELECT value FROM json_each(?7)) AND (?3 IS NULL OR TRUE)",
    ">",
    "ASC"
);

/// The maps of several matches, by match and number.
pub async fn maps_for(
    ex: impl SqliteExecutor<'_>,
    match_ids: &[MatchId],
) -> DbResult<Vec<MatchMap>> {
    let ids: Vec<i64> = match_ids.iter().map(|m| m.0).collect();
    sqlx::query(concat!(
        "SELECT ",
        map_columns!(),
        " FROM maps WHERE match_id IN (SELECT value FROM json_each(?)) \
          ORDER BY match_id, number"
    ))
    .bind(sqlx::types::Json(ids))
    .try_map(map_from_row)
    .fetch_all(ex)
    .await
}
