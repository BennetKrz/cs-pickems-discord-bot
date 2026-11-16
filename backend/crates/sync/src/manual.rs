//! The `manual` source and admin match actions (T05).
//!
//! Admin overrides are stored like any other source's data: one `manual`
//! record per match (`source_records`, a [`SrcMatch`] in canonical A/B
//! orientation, without teams) plus a link with `external_id` = the
//! canonical match ID. `manual` comes first in every precedence group, so an
//! override wins over every source and survives the next sync. Clearing an
//! override removes its field from the record, so the next source in order
//! applies again; if no source has a value, a score goes back to neutral.
//!
//! Void, unvoid and reopen change admin state on the match itself. Every
//! action runs in one transaction, writes `audit_log` with the acting
//! Discord user, and publishes its domain events after the commit.
//! See `docs/adr/0005-manual-overrides-and-fixtures.md`.

use std::sync::Arc;

use chrono::{DateTime, Utc};
use domain::{DomainEvent, EntityType, MapStatus, Match, MatchId, MatchMap, MatchStatus, Side};
use serde_json::{Value, json};
use sources::{SrcMap, SrcMatch};
use sqlx::SqliteConnection;
use store::audit::{self, AuditRecord};
use store::refs::{self, ExternalRef};
use store::{matches, provenance};

use crate::precedence::{FieldGroup, MANUAL};
use crate::reconcile::{ReconcileError, Reconciler, Report, match_events, record};

/// Errors of an admin action.
#[derive(Debug, thiserror::Error)]
pub enum ManualError {
    #[error("match {0} does not exist")]
    NotFound(MatchId),
    /// The request itself is wrong, e.g. a score impossible for the format.
    #[error("{0}")]
    Invalid(String),
    /// The request is fine but the match's state does not allow it.
    #[error("{0}")]
    Conflict(String),
    #[error(transparent)]
    Reconcile(#[from] ReconcileError),
}

impl From<sqlx::Error> for ManualError {
    fn from(e: sqlx::Error) -> Self {
        ManualError::Reconcile(e.into())
    }
}

type Result<T, E = ManualError> = std::result::Result<T, E>;

/// One override: a value for one field group of a match.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Override {
    /// Match status. Missing timestamps keep their current value, or become
    /// `now` when the match moves to live or finished.
    Status {
        status: MatchStatus,
        started_at: Option<DateTime<Utc>>,
        ended_at: Option<DateTime<Utc>>,
    },
    Schedule {
        scheduled_at: DateTime<Utc>,
    },
    /// Maps won `(a, b)` in canonical order; the winner follows from it.
    Series {
        score: (u8, u8),
    },
    /// One map. A finished map needs rounds with a winner; an upcoming or
    /// unplayed map has no rounds.
    Map {
        number: u8,
        status: MapStatus,
        rounds: Option<(u16, u16)>,
    },
}

/// The field an override sets, for clearing it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OverrideField {
    Status,
    Schedule,
    Series,
    Map(u8),
}

impl OverrideField {
    /// The provenance and audit name: `status`, `schedule`, `series` or
    /// `map_rounds.N`.
    pub fn name(self) -> String {
        match self {
            OverrideField::Status => FieldGroup::Status.to_string(),
            OverrideField::Schedule => FieldGroup::Schedule.to_string(),
            OverrideField::Series => FieldGroup::Series.to_string(),
            OverrideField::Map(n) => format!("{}.{n}", FieldGroup::MapRounds),
        }
    }
}

impl Override {
    pub fn field(&self) -> OverrideField {
        match self {
            Override::Status { .. } => OverrideField::Status,
            Override::Schedule { .. } => OverrideField::Schedule,
            Override::Series { .. } => OverrideField::Series,
            Override::Map { number, .. } => OverrideField::Map(*number),
        }
    }
}

/// The match after an action, and the domain events it published.
#[derive(Debug, Clone, PartialEq)]
pub struct Outcome {
    pub current: Match,
    pub maps: Vec<MatchMap>,
    pub events: Vec<DomainEvent>,
}

/// Admin overrides, void and reopen. Cheap to clone.
#[derive(Clone)]
pub struct ManualService {
    reconciler: Arc<Reconciler>,
}

impl ManualService {
    pub fn new(reconciler: Arc<Reconciler>) -> Self {
        Self { reconciler }
    }

    /// The current manual overrides of a match, if any.
    pub async fn overrides(&self, id: MatchId) -> Result<Option<SrcMatch>> {
        let mut c = self.reconciler.pool().acquire().await?;
        load_manual(&mut c, id).await
    }

    /// Sets an override and re-merges the match.
    pub async fn set(
        &self,
        id: MatchId,
        ov: Override,
        actor: &str,
        now: DateTime<Utc>,
    ) -> Result<Outcome> {
        check_actor(actor)?;
        self.in_tx(id, async |c, cur, _maps, out| {
            let mut dto = load_manual(c, id)
                .await?
                .unwrap_or_else(|| SrcMatch::new(id.to_string(), cur.fields.event_id.to_string()));
            let field = ov.field();
            let before = field_value(&dto, field);
            apply(&mut dto, cur, &ov, now)?;
            let after = field_value(&dto, field);
            save_manual(c, id, &dto, now).await?;
            audit_change(
                c,
                actor,
                "manual_override_set",
                id,
                field,
                before,
                after,
                now,
            )
            .await?;
            self.remerge(c, id, cur, None, now, out).await
        })
        .await
    }

    /// Clears an override and re-merges the match from the other sources.
    /// Clearing a field without an override changes nothing.
    pub async fn clear(
        &self,
        id: MatchId,
        field: OverrideField,
        actor: &str,
        now: DateTime<Utc>,
    ) -> Result<Outcome> {
        check_actor(actor)?;
        self.in_tx(id, async |c, cur, _maps, out| {
            let Some(mut dto) = load_manual(c, id).await? else {
                return Ok(());
            };
            let before = field_value(&dto, field);
            if before.is_none() {
                return Ok(());
            }
            remove(&mut dto, field);
            if is_empty(&dto) {
                let ext = id.to_string();
                refs::unlink(&mut *c, MANUAL, EntityType::Match, &ext).await?;
                refs::delete_record(&mut *c, MANUAL, EntityType::Match, &ext).await?;
            } else {
                save_manual(c, id, &dto, now).await?;
            }
            audit_change(
                c,
                actor,
                "manual_override_clear",
                id,
                field,
                before,
                None,
                now,
            )
            .await?;
            self.remerge(c, id, cur, Some(field), now, out).await
        })
        .await
    }

    /// Voids the match: no points for anyone, picks are kept.
    pub async fn void(&self, id: MatchId, actor: &str, now: DateTime<Utc>) -> Result<Outcome> {
        self.set_voided(id, true, actor, now).await
    }

    /// Takes a void back.
    pub async fn unvoid(&self, id: MatchId, actor: &str, now: DateTime<Utc>) -> Result<Outcome> {
        self.set_voided(id, false, actor, now).await
    }

    async fn set_voided(
        &self,
        id: MatchId,
        voided: bool,
        actor: &str,
        now: DateTime<Utc>,
    ) -> Result<Outcome> {
        check_actor(actor)?;
        self.in_tx(id, async |c, cur, _maps, out| {
            if cur.fields.voided == voided {
                return Err(ManualError::Conflict(if voided {
                    format!("match {id} is already voided")
                } else {
                    format!("match {id} is not voided")
                }));
            }
            let mut next = cur.clone();
            next.fields.voided = voided;
            matches::update(&mut *c, &next).await?;
            let (action, event) = if voided {
                ("match_void", DomainEvent::MatchVoided { match_id: id })
            } else {
                ("match_unvoid", DomainEvent::MatchUnvoided { match_id: id })
            };
            audit::append(
                &mut *c,
                &AuditRecord {
                    actor: actor.to_owned(),
                    action: action.to_owned(),
                    entity_type: EntityType::Match.as_str().to_owned(),
                    entity_id: Some(id.to_string()),
                    before: Some(json!({ "voided": !voided })),
                    after: Some(json!({ "voided": voided })),
                    at: now,
                },
            )
            .await?;
            out.push(event);
            Ok(())
        })
        .await
    }

    /// Unlocks the picks of a locked match that has not started yet, e.g.
    /// after a long postponement. The new lock time is the scheduled time,
    /// which must be in the future: set a schedule override first if needed.
    pub async fn reopen(&self, id: MatchId, actor: &str, now: DateTime<Utc>) -> Result<Outcome> {
        check_actor(actor)?;
        self.in_tx(id, async |c, cur, _maps, out| {
            let f = &cur.fields;
            if !f.locked {
                return Err(ManualError::Conflict(format!(
                    "the picks of match {id} are not locked"
                )));
            }
            if !matches!(f.status, MatchStatus::Scheduled | MatchStatus::Postponed) {
                return Err(ManualError::Conflict(format!(
                    "match {id} is {}; picks can only be reopened before it is live",
                    f.status
                )));
            }
            let Some(at) = f.scheduled_at.filter(|&at| at > now) else {
                return Err(ManualError::Conflict(format!(
                    "match {id} has no scheduled time in the future; set a new schedule first"
                )));
            };
            let mut next = cur.clone();
            next.fields.locked = false;
            next.fields.lock_at = Some(at);
            matches::update(&mut *c, &next).await?;
            audit::append(
                &mut *c,
                &AuditRecord {
                    actor: actor.to_owned(),
                    action: "picks_reopen".to_owned(),
                    entity_type: EntityType::Match.as_str().to_owned(),
                    entity_id: Some(id.to_string()),
                    before: Some(json!({ "locked": true, "lock_at": f.lock_at })),
                    after: Some(json!({ "locked": false, "lock_at": at })),
                    at: now,
                },
            )
            .await?;
            out.push(DomainEvent::PicksReopened { match_id: id });
            Ok(())
        })
        .await
    }

    // ---- internals --------------------------------------------------------

    /// Runs `f` on the current match in one transaction; commits, publishes
    /// the events, and returns the match as stored afterwards.
    async fn in_tx(
        &self,
        id: MatchId,
        f: impl AsyncFnOnce(
            &mut SqliteConnection,
            &Match,
            &[MatchMap],
            &mut Vec<DomainEvent>,
        ) -> Result<()>,
    ) -> Result<Outcome> {
        let mut tx = self.reconciler.pool().begin().await?;
        let cur = matches::get(&mut *tx, id)
            .await?
            .ok_or(ManualError::NotFound(id))?;
        let maps = matches::list_maps(&mut *tx, id).await?;
        let mut events = Vec::new();
        f(&mut tx, &cur, &maps, &mut events).await?;
        let current = matches::get(&mut *tx, id)
            .await?
            .ok_or(ManualError::NotFound(id))?;
        let maps = matches::list_maps(&mut *tx, id).await?;
        tx.commit().await?;
        self.reconciler.bus().publish(events.iter().cloned());
        Ok(Outcome {
            current,
            maps,
            events,
        })
    }

    /// Re-merges the match after its manual record changed. If a cleared
    /// field has no other source, its value goes back to neutral (score
    /// 0:0, map upcoming) and its provenance is removed. Events are the
    /// difference between `before` and the final state.
    async fn remerge(
        &self,
        c: &mut SqliteConnection,
        id: MatchId,
        before: &Match,
        cleared: Option<OverrideField>,
        now: DateTime<Utc>,
        out: &mut Vec<DomainEvent>,
    ) -> Result<()> {
        let before_maps = matches::list_maps(&mut *c, id).await?;
        let mut rep = Report::default();
        let mut merge_events = Vec::new();
        self.reconciler
            .refresh_match_in(c, id, now, &mut rep, &mut merge_events)
            .await?;
        if let Some(field) = cleared {
            reset_if_unsourced(c, id, field).await?;
        }
        let after = matches::get(&mut *c, id)
            .await?
            .ok_or(ManualError::NotFound(id))?;
        let after_maps = matches::list_maps(&mut *c, id).await?;
        out.extend(match_events(before, &before_maps, &after, &after_maps));
        Ok(())
    }
}

fn check_actor(actor: &str) -> Result<()> {
    if actor.is_empty() || !actor.bytes().all(|b| b.is_ascii_digit()) {
        return Err(ManualError::Invalid(format!(
            "actor `{actor}` is not a Discord user ID"
        )));
    }
    Ok(())
}

async fn load_manual(c: &mut SqliteConnection, id: MatchId) -> Result<Option<SrcMatch>> {
    Ok(record::load::<SrcMatch>(c, MANUAL, EntityType::Match, &id.to_string()).await?)
}

/// Stores the manual record and makes sure it is linked to the match.
async fn save_manual(
    c: &mut SqliteConnection,
    id: MatchId,
    dto: &SrcMatch,
    now: DateTime<Utc>,
) -> Result<()> {
    let ext = id.to_string();
    record::store(c, MANUAL, EntityType::Match, &ext, dto, &Value::Null, now).await?;
    if refs::resolve(&mut *c, MANUAL, EntityType::Match, &ext)
        .await?
        .is_none()
    {
        refs::link(
            &mut *c,
            &ExternalRef {
                entity_type: EntityType::Match,
                entity_id: id.0,
                source: MANUAL.to_owned(),
                external_id: ext,
            },
        )
        .await?;
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn audit_change(
    c: &mut SqliteConnection,
    actor: &str,
    action: &str,
    id: MatchId,
    field: OverrideField,
    before: Option<Value>,
    after: Option<Value>,
    now: DateTime<Utc>,
) -> Result<()> {
    let name = field.name();
    audit::append(
        &mut *c,
        &AuditRecord {
            actor: actor.to_owned(),
            action: action.to_owned(),
            entity_type: EntityType::Match.as_str().to_owned(),
            entity_id: Some(id.to_string()),
            before: Some(json!({ "field": name, "value": before })),
            after: Some(json!({ "field": name, "value": after })),
            at: now,
        },
    )
    .await?;
    Ok(())
}

/// The override value of one field in a manual record, as JSON.
fn field_value(dto: &SrcMatch, field: OverrideField) -> Option<Value> {
    match field {
        OverrideField::Status => dto.status.map(
            |s| json!({ "status": s, "started_at": dto.started_at, "ended_at": dto.ended_at }),
        ),
        OverrideField::Schedule => dto.scheduled_at.map(|t| json!({ "scheduled_at": t })),
        OverrideField::Series => dto.score.map(|s| json!({ "score": [s.0, s.1] })),
        OverrideField::Map(n) => dto
            .maps
            .iter()
            .find(|m| m.number == n)
            .map(|m| serde_json::to_value(m).expect("maps serialize")),
    }
}

/// Validates an override against the match and writes it into the record.
fn apply(dto: &mut SrcMatch, cur: &Match, ov: &Override, now: DateTime<Utc>) -> Result<()> {
    let f = &cur.fields;
    let teams_known = f.team_a_id.is_some() && f.team_b_id.is_some();
    match *ov {
        Override::Status {
            status,
            started_at,
            ended_at,
        } => {
            let has_started = matches!(
                status,
                MatchStatus::Live | MatchStatus::Finished | MatchStatus::Canceled
            );
            let started_at = started_at.or(if has_started {
                f.started_at
                    .or((status != MatchStatus::Canceled).then_some(now))
            } else {
                None
            });
            let ended_at = ended_at.or(match status {
                MatchStatus::Finished => f.ended_at.or(Some(now)),
                MatchStatus::Canceled => f.ended_at,
                _ => None,
            });
            if let (Some(s), Some(e)) = (started_at, ended_at)
                && e < s
            {
                return Err(ManualError::Invalid(
                    "ended_at is before started_at".to_owned(),
                ));
            }
            dto.status = Some(status);
            dto.started_at = started_at;
            dto.ended_at = ended_at;
        }
        Override::Schedule { scheduled_at } => dto.scheduled_at = Some(scheduled_at),
        Override::Series { score: (a, b) } => {
            let wins = f.format.wins_needed();
            if a > wins || b > wins || (a == wins && b == wins) {
                return Err(ManualError::Invalid(format!(
                    "{a}:{b} is not a possible score for a {}",
                    f.format
                )));
            }
            if !teams_known {
                return Err(ManualError::Conflict(
                    "both teams must be known before a score can be set".to_owned(),
                ));
            }
            dto.score = Some((a, b));
            dto.winner = None;
        }
        Override::Map {
            number,
            status,
            rounds,
        } => {
            let max = f.format.max_maps();
            if !(1..=max).contains(&number) {
                return Err(ManualError::Invalid(format!(
                    "map {number} does not exist in a {}",
                    f.format
                )));
            }
            let winner = match (status, rounds) {
                (MapStatus::Finished, Some((ra, rb))) if ra != rb => {
                    Some(if ra > rb { Side::A } else { Side::B })
                }
                (MapStatus::Finished, _) => {
                    return Err(ManualError::Invalid(
                        "a finished map needs rounds with a winner".to_owned(),
                    ));
                }
                (MapStatus::Live, _) => None,
                (MapStatus::Upcoming | MapStatus::NotPlayed, None) => None,
                (MapStatus::Upcoming | MapStatus::NotPlayed, Some(_)) => {
                    return Err(ManualError::Invalid(format!(
                        "a map that is {status} has no rounds"
                    )));
                }
            };
            if rounds.is_some() && !teams_known {
                return Err(ManualError::Conflict(
                    "both teams must be known before a score can be set".to_owned(),
                ));
            }
            dto.maps.retain(|m| m.number != number);
            dto.maps.push(SrcMap {
                status: Some(status),
                rounds,
                winner,
                ..SrcMap::new(number)
            });
            dto.maps.sort_by_key(|m| m.number);
        }
    }
    Ok(())
}

/// Removes one field from a manual record.
fn remove(dto: &mut SrcMatch, field: OverrideField) {
    match field {
        OverrideField::Status => {
            dto.status = None;
            dto.started_at = None;
            dto.ended_at = None;
        }
        OverrideField::Schedule => dto.scheduled_at = None,
        OverrideField::Series => {
            dto.score = None;
            dto.winner = None;
        }
        OverrideField::Map(n) => dto.maps.retain(|m| m.number != n),
    }
}

/// Whether a manual record has no override left.
fn is_empty(dto: &SrcMatch) -> bool {
    dto.status.is_none()
        && dto.scheduled_at.is_none()
        && dto.score.is_none()
        && dto.winner.is_none()
        && dto.maps.is_empty()
}

/// After clearing `field`: if its provenance still names `manual`, no other
/// source has a value. Reset scores to neutral and forget the provenance.
/// Status and schedule keep their value, as there is no neutral one.
async fn reset_if_unsourced(
    c: &mut SqliteConnection,
    id: MatchId,
    field: OverrideField,
) -> Result<()> {
    let name = field.name();
    let stale = provenance::list_for(&mut *c, EntityType::Match, id.0)
        .await?
        .into_iter()
        .any(|p| p.field == name && p.source == MANUAL);
    if !stale {
        return Ok(());
    }
    match field {
        OverrideField::Series => {
            let mut m = matches::get(&mut *c, id)
                .await?
                .ok_or(ManualError::NotFound(id))?;
            m.fields.maps_won_a = 0;
            m.fields.maps_won_b = 0;
            m.fields.winner_team_id = None;
            matches::update(&mut *c, &m).await?;
        }
        OverrideField::Map(n) => {
            let maps = matches::list_maps(&mut *c, id).await?;
            if let Some(map) = maps.into_iter().find(|m| m.number == n) {
                matches::upsert_map(
                    &mut *c,
                    &MatchMap {
                        status: MapStatus::Upcoming,
                        rounds_a: None,
                        rounds_b: None,
                        winner_team_id: None,
                        ..map
                    },
                )
                .await?;
            }
        }
        OverrideField::Status | OverrideField::Schedule => {}
    }
    provenance::clear(&mut *c, EntityType::Match, id.0, &name).await?;
    Ok(())
}
