//! Reads for a member's profile and pick history (T15). The numbers in the
//! profile header come from `leaderboard::load`, so they equal the
//! leaderboard's; this module lists the matches a member picked, by
//! segment, and counts them. See `docs/adr/0018-player-profile.md`.
//!
//! Unlike the match list (T12), the history covers every event, followed or
//! not, like the leaderboard does.

use chrono::{DateTime, Utc};
use domain::{EventId, MatchId};
use sqlx::SqliteExecutor;

use crate::matches::{MatchListItem, list_item_from_row};
use crate::{DbResult, Ts};

/// A segment of a member's pick history.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PickSegment {
    /// The match has not started: `scheduled` or `postponed`, not voided.
    /// By scheduled start, ascending (no start time first).
    Open,
    /// `live`, not voided. By start, ascending.
    Live,
    /// `finished`, `canceled`, or voided in any status. By end (else start,
    /// else scheduled time), latest first; no time window.
    Settled,
}

/// What [`list_picks`] lists.
#[derive(Debug, Clone)]
pub struct PickQuery<'a> {
    pub guild_id: &'a str,
    /// Whose picks.
    pub user_id: &'a str,
    pub segment: PickSegment,
    pub event_id: Option<EventId>,
    /// Keyset cursor: list only matches after this `(sort_at, id)`.
    pub after: Option<(DateTime<Utc>, MatchId)>,
    pub limit: i64,
}

/// Only matches the member (`?7`) picked in the guild (`?1`).
macro_rules! picked_sql {
    ($key:literal, $where:literal, $cmp:literal, $dir:literal) => {
        list_sql!(
            @from "JOIN picks up ON up.match_id = m.id AND up.guild_id = ?1 AND up.user_id = ?7 ",
            $key,
            $where,
            $cmp,
            $dir
        )
    };
}

const OPEN_SQL: &str = picked_sql!(
    "coalesce(scheduled_at, '')",
    "m.status IN ('scheduled', 'postponed') AND NOT m.voided AND (?3 IS NULL OR TRUE)",
    ">",
    "ASC"
);
const LIVE_SQL: &str = picked_sql!(
    "coalesce(started_at, scheduled_at, '')",
    "m.status = 'live' AND NOT m.voided AND (?3 IS NULL OR TRUE)",
    ">",
    "ASC"
);
const SETTLED_SQL: &str = picked_sql!(
    "coalesce(ended_at, started_at, scheduled_at, '')",
    "(m.status IN ('finished', 'canceled') OR m.voided) AND (?3 IS NULL OR TRUE)",
    "<",
    "DESC"
);

/// The matches a member picked in a guild, one segment, with event, stage,
/// teams and the guild's pick count. `sort_at` is `None` for a match
/// without the segment's time.
pub async fn list_picks(
    ex: impl SqliteExecutor<'_>,
    q: &PickQuery<'_>,
) -> DbResult<Vec<MatchListItem>> {
    let sql = match q.segment {
        PickSegment::Open => OPEN_SQL,
        PickSegment::Live => LIVE_SQL,
        PickSegment::Settled => SETTLED_SQL,
    };
    sqlx::query(sql)
        .bind(q.guild_id)
        .bind(q.event_id.map(|e| e.0))
        .bind(None::<Ts>)
        .bind(q.after.map(|(t, _)| Ts(t)))
        .bind(q.after.map(|(_, id)| id.0))
        .bind(q.limit)
        .bind(q.user_id)
        .try_map(list_item_from_row)
        .fetch_all(ex)
        .await
}

/// How many picks a member has per segment.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PickCounts {
    pub open: i64,
    pub live: i64,
    pub settled: i64,
}

impl PickCounts {
    pub fn total(&self) -> i64 {
        self.open + self.live + self.settled
    }
}

/// A member's picks per segment in a guild, optionally of one event. Uses
/// the same conditions as [`list_picks`].
pub async fn pick_counts(
    ex: impl SqliteExecutor<'_>,
    guild_id: &str,
    user_id: &str,
    event_id: Option<EventId>,
) -> DbResult<PickCounts> {
    let (open, live, settled): (i64, i64, i64) = sqlx::query_as(
        "SELECT \
           coalesce(sum(m.status IN ('scheduled', 'postponed') AND NOT m.voided), 0), \
           coalesce(sum(m.status = 'live' AND NOT m.voided), 0), \
           coalesce(sum(m.status IN ('finished', 'canceled') OR m.voided), 0) \
         FROM picks p JOIN matches m ON m.id = p.match_id \
         WHERE p.guild_id = ?1 AND p.user_id = ?2 AND (?3 IS NULL OR m.event_id = ?3)",
    )
    .bind(guild_id)
    .bind(user_id)
    .bind(event_id.map(|e| e.0))
    .fetch_one(ex)
    .await?;
    Ok(PickCounts {
        open,
        live,
        settled,
    })
}

/// The user belongs to the guild: they signed in there (`guild_members`),
/// or they have picks there.
pub async fn is_member(
    ex: impl SqliteExecutor<'_>,
    guild_id: &str,
    user_id: &str,
) -> DbResult<bool> {
    sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM guild_members WHERE guild_id = ?1 AND user_id = ?2) \
             OR EXISTS (SELECT 1 FROM picks WHERE guild_id = ?1 AND user_id = ?2)",
    )
    .bind(guild_id)
    .bind(user_id)
    .fetch_one(ex)
    .await
}
