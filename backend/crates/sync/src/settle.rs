//! Settlement (T11): turns match results into points.
//!
//! Settlement is a **recompute, not a delta**. For a match it scores every
//! pick from the current match state with `domain::scoring::score` and
//! upserts `pick_results`. Running it again, after a correction, or after
//! a replay, gives the same rows. A row is only written when its points,
//! breakdown, state or rules version changed, so a second run writes
//! nothing at all.
//!
//! - **Triggers:** every domain event re-settles its match. The state
//!   decides the outcome: a live match gives provisional points for its
//!   finished maps, a finished one final points, a canceled or voided one
//!   void results, and a match that is open again (reopen) loses its
//!   results.
//! - **Sweep:** at start, on a lagged event stream, and every
//!   `settle_every`, it re-settles the matches `store::scoring::matches_to_settle`
//!   finds: missed events, live matches, and parts waiting for data, which
//!   become void [`DATA_DEADLINE_HOURS`] after the match ended.
//! - **Rules:** a match is settled with the guild's rules version that was
//!   active when it locked, pinned at its first settlement.
//!   [`rescore_all`] re-pins every match to the current rules on purpose.
//!
//! See `docs/adr/0011-scoring-and-settlement.md`.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use chrono::{DateTime, Duration, Utc};
use domain::scoring::{self, DATA_DEADLINE_HOURS, MatchResult, ScoringRules};
use domain::{Clock, DomainEvent, Match, MatchId, MatchStatus, Pick, PickId};
use sqlx::SqliteConnection;
use store::SqlitePool;
use store::picks::{self, PickResult};
use store::scoring::{self as rules_store, MatchPin};
use tokio::sync::broadcast::{self, error::RecvError};
use tokio_util::sync::CancellationToken;

/// Which rules version settles a match.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RulesMode {
    /// The pinned version; on a match's first settlement, the version that
    /// was active when it locked.
    Pinned,
    /// The guild's current version, re-pinned (`app rescore --all`).
    Current,
}

/// What a settlement run did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SettleReport {
    /// Matches looked at.
    pub matches: usize,
    /// Picks scored.
    pub picks: usize,
    /// `pick_results` rows inserted or changed.
    pub written: usize,
    /// `pick_results` rows deleted (reopened matches).
    pub deleted: usize,
}

impl SettleReport {
    fn add(&mut self, o: SettleReport) {
        self.matches += o.matches;
        self.picks += o.picks;
        self.written += o.written;
        self.deleted += o.deleted;
    }
}

#[derive(Debug, thiserror::Error)]
pub enum SettleError {
    #[error(transparent)]
    Db(#[from] sqlx::Error),
    /// Stored rules that don't parse. `app rules set` validates them, so
    /// this means the database was edited by hand.
    #[error("scoring rules version {version} of guild {guild_id} are invalid: {reason}")]
    InvalidRules {
        guild_id: String,
        version: i64,
        reason: String,
    },
}

type Result<T, E = SettleError> = std::result::Result<T, E>;

/// Recomputes the results of every pick of one match, in one transaction.
pub async fn settle_match(
    pool: &SqlitePool,
    id: MatchId,
    now: DateTime<Utc>,
    mode: RulesMode,
) -> Result<SettleReport> {
    let mut tx = store::begin_write(pool).await?;
    let report = settle_in(&mut tx, id, now, mode).await?;
    tx.commit().await?;
    if report.written > 0 || report.deleted > 0 {
        tracing::info!(
            match_id = %id,
            picks = report.picks,
            written = report.written,
            deleted = report.deleted,
            "match settled"
        );
    }
    Ok(report)
}

async fn settle_in(
    c: &mut SqliteConnection,
    id: MatchId,
    now: DateTime<Utc>,
    mode: RulesMode,
) -> Result<SettleReport> {
    let mut report = SettleReport {
        matches: 1,
        ..SettleReport::default()
    };
    let Some(m) = store::matches::get(&mut *c, id).await? else {
        return Ok(report);
    };
    let f = &m.fields;

    // Open again (an admin reopened it): nothing is settled, and the next
    // lock picks the rules anew.
    if !f.voided && matches!(f.status, MatchStatus::Scheduled | MatchStatus::Postponed) {
        report.deleted = usize::try_from(picks::delete_results_for_match(&mut *c, id).await?)
            .unwrap_or(usize::MAX);
        rules_store::delete_pins_for_match(&mut *c, id).await?;
        return Ok(report);
    }

    let all = picks::list_for_match_all_guilds(&mut *c, id).await?;
    if all.is_empty() {
        return Ok(report);
    }
    let maps = store::matches::list_maps(&mut *c, id).await?;
    let existing: HashMap<PickId, PickResult> = picks::results_for_match(&mut *c, id)
        .await?
        .into_iter()
        .map(|r| (r.pick_id, r))
        .collect();
    let mut by_guild: BTreeMap<&str, Vec<&Pick>> = BTreeMap::new();
    for p in &all {
        by_guild.entry(p.guild_id.as_str()).or_default().push(p);
    }

    for (guild_id, guild_picks) in by_guild {
        let (pin, rules) = pin_rules(c, guild_id, &m, now, mode).await?;
        let deadline_from = f.ended_at.or(pin.finished_seen_at);
        let deadline_passed =
            deadline_from.is_some_and(|t| now >= t + Duration::hours(DATA_DEADLINE_HOURS));
        let result = MatchResult::from_match(&m, &maps, deadline_passed);

        for p in guild_picks {
            report.picks += 1;
            let scored = scoring::score(&p.parts, &result, &rules);
            let row = PickResult {
                pick_id: p.id,
                points: i64::from(scored.points),
                breakdown: serde_json::to_value(&scored.breakdown)
                    .expect("a breakdown always serializes"),
                rules_version: pin.rules_version,
                settled_at: now,
                state: scored.state,
                pending_parts: i64::from(scored.pending_parts),
            };
            if existing
                .get(&p.id)
                .is_some_and(|old| same_result(old, &row))
            {
                continue;
            }
            picks::upsert_result(&mut *c, &row).await?;
            report.written += 1;
        }
    }
    Ok(report)
}

/// Whether two results are equal apart from when they were settled.
fn same_result(a: &PickResult, b: &PickResult) -> bool {
    a.points == b.points
        && a.breakdown == b.breakdown
        && a.rules_version == b.rules_version
        && a.state == b.state
        && a.pending_parts == b.pending_parts
}

/// The pin of a match in a guild, created or updated as needed, and the
/// rules it names.
async fn pin_rules(
    c: &mut SqliteConnection,
    guild_id: &str,
    m: &Match,
    now: DateTime<Utc>,
    mode: RulesMode,
) -> Result<(MatchPin, ScoringRules)> {
    let old = rules_store::get_pin(&mut *c, guild_id, m.id).await?;
    let version = match (mode, &old) {
        (RulesMode::Current, _) => rules_store::current_rules(&mut *c, guild_id).await?.version,
        (RulesMode::Pinned, Some(p)) => p.rules_version,
        (RulesMode::Pinned, None) => {
            rules_store::version_at(&mut *c, guild_id, lock_moment(m).unwrap_or(now)).await?
        }
    };
    let finished_seen_at = old
        .as_ref()
        .and_then(|p| p.finished_seen_at)
        .or((m.fields.status == MatchStatus::Finished).then_some(now));
    let pin = MatchPin {
        guild_id: guild_id.to_owned(),
        match_id: m.id,
        rules_version: version,
        finished_seen_at,
    };
    if old.as_ref() != Some(&pin) {
        rules_store::put_pin(&mut *c, &pin).await?;
    }
    let rules = load_rules(c, guild_id, version).await?;
    Ok((pin, rules))
}

/// When a match locked: at `lock_at` or when it went live, whichever came
/// first (T10).
fn lock_moment(m: &Match) -> Option<DateTime<Utc>> {
    match (m.fields.lock_at, m.fields.started_at) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (a, b) => a.or(b),
    }
}

async fn load_rules(
    c: &mut SqliteConnection,
    guild_id: &str,
    version: i64,
) -> Result<ScoringRules> {
    let invalid = |reason: String| SettleError::InvalidRules {
        guild_id: guild_id.to_owned(),
        version,
        reason,
    };
    let Some(v) = rules_store::rules_of(&mut *c, guild_id, version).await? else {
        return Err(invalid("the version does not exist".to_owned()));
    };
    let rules = match v.rules {
        None => ScoringRules::default(),
        Some(json) => serde_json::from_value(json).map_err(|e| invalid(e.to_string()))?,
    };
    rules.validate().map_err(|e| invalid(e.to_string()))?;
    Ok(rules)
}

/// Re-settles every match whose results are missing, incomplete or out of
/// step (see `store::scoring::matches_to_settle`). A match that fails is
/// logged and skipped.
pub async fn sweep(pool: &SqlitePool, now: DateTime<Utc>) -> Result<SettleReport> {
    let ids = rules_store::matches_to_settle(pool).await?;
    Ok(settle_each(pool, &ids, now, RulesMode::Pinned).await)
}

/// Re-settles every match with picks using each guild's **current** rules
/// (`app rescore --all`).
pub async fn rescore_all(pool: &SqlitePool, now: DateTime<Utc>) -> Result<SettleReport> {
    let ids = rules_store::matches_with_picks(pool).await?;
    Ok(settle_each(pool, &ids, now, RulesMode::Current).await)
}

async fn settle_each(
    pool: &SqlitePool,
    ids: &[MatchId],
    now: DateTime<Utc>,
    mode: RulesMode,
) -> SettleReport {
    let mut total = SettleReport::default();
    for &id in ids {
        match settle_match(pool, id, now, mode).await {
            Ok(r) => total.add(r),
            Err(e) => tracing::warn!(match_id = %id, "settlement failed: {e}"),
        }
    }
    total
}

/// Runs settlement until `cancel` fires: a sweep at start and every
/// `every`, a settle of the event's match for each domain event, and a
/// sweep when the event stream lagged. Subscribe `events` before the
/// components that publish start, so nothing is missed.
pub async fn run_settler(
    pool: SqlitePool,
    clock: Arc<dyn Clock>,
    mut events: broadcast::Receiver<DomainEvent>,
    every: std::time::Duration,
    cancel: CancellationToken,
) {
    // The first tick fires at once: the catch-up sweep.
    let mut interval = tokio::time::interval(every);
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut open = true;
    loop {
        tokio::select! {
            () = cancel.cancelled() => break,
            _ = interval.tick() => run_sweep(&pool, clock.now()).await,
            ev = events.recv(), if open => match ev {
                Ok(e) => {
                    let id = e.match_id();
                    if let Err(err) = settle_match(&pool, id, clock.now(), RulesMode::Pinned).await {
                        tracing::warn!(match_id = %id, event = ?e, "settlement failed: {err}");
                    }
                }
                Err(RecvError::Lagged(n)) => {
                    tracing::warn!(missed = n, "settlement lagged behind domain events; sweeping");
                    run_sweep(&pool, clock.now()).await;
                }
                Err(RecvError::Closed) => {
                    tracing::warn!("domain event bus closed; settlement continues by sweep only");
                    open = false;
                }
            },
        }
    }
}

async fn run_sweep(pool: &SqlitePool, now: DateTime<Utc>) {
    match sweep(pool, now).await {
        Ok(r) if r.written > 0 || r.deleted > 0 => tracing::info!(
            matches = r.matches,
            written = r.written,
            deleted = r.deleted,
            "settlement sweep"
        ),
        Ok(_) => {}
        Err(e) => tracing::warn!("settlement sweep failed: {e}"),
    }
}
