//! The reconciler: turns source DTOs into canonical data.
//!
//! For every DTO it stores the payload in `source_records`, resolves the
//! entity through `external_refs`, and then re-merges the canonical entity
//! from the latest records of **all** sources linked to it, by field
//! precedence. Only real changes are written, and only those produce domain
//! events, which are published after the commit.
//!
//! Who creates entities: only the primary source. Another source's entity
//! is linked by (1) an existing link, (2) for teams an exact normalized name
//! or acronym, (3) for matches the same event, teams and a time within ±2 h;
//! anything else waits in the unlinked queue for an admin. The `manual`
//! source never creates and is never queued: it applies only through links
//! that T05 sets. See `docs/adr/0004-source-adapters-reconciliation.md`.

mod diff;
pub mod identity;
pub mod merge;
pub mod record;

use std::ops::AddAssign;

use chrono::{DateTime, Utc};
use domain::{
    DomainEvent, EntityType, Event, EventId, Feed, Match, MatchId, NewStage, Stage, StageId, Team,
    TeamId, UnlinkedReason,
};
use serde_json::{Value, json};
use sources::{Sourced, SrcEvent, SrcMatch, SrcStage, SrcTeam};
use sqlx::SqliteConnection;
use store::SqlitePool;
use store::audit::{self, AuditRecord};
use store::refs::{self, ExternalRef};
use store::unlinked::{self, UnlinkedRef};
use store::{events, matches, provenance, teams};

use crate::bus::EventBus;
use crate::precedence::{FieldGroup, MANUAL, Precedence};
pub use diff::match_events;
pub use merge::{MatchView, MergedMatch};

/// Errors of the reconciler.
#[derive(Debug, thiserror::Error)]
pub enum ReconcileError {
    #[error("database error: {0}")]
    Db(#[from] sqlx::Error),
    #[error("stored payload {source_name}/{entity_type}/{external_id} is invalid: {msg}")]
    Payload {
        source_name: String,
        entity_type: EntityType,
        external_id: String,
        msg: String,
    },
    #[error("{0}")]
    Invalid(String),
}

type Result<T, E = ReconcileError> = std::result::Result<T, E>;

/// What a reconciler call did, summed over all entities it touched.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Report {
    /// Canonical entities created.
    pub created: u32,
    /// External IDs linked to existing entities (identity rules or admin).
    pub linked: u32,
    /// Existing entities whose canonical values changed.
    pub updated: u32,
    /// Reports that went to (or stayed in) the unlinked queue.
    pub queued: u32,
    /// Records that could not be reconciled (logged as warnings).
    pub failed: u32,
    /// Domain events published.
    pub events: u32,
}

impl AddAssign for Report {
    fn add_assign(&mut self, o: Self) {
        self.created += o.created;
        self.linked += o.linked;
        self.updated += o.updated;
        self.queued += o.queued;
        self.failed += o.failed;
        self.events += o.events;
    }
}

type Conn = SqliteConnection;
type Events = Vec<DomainEvent>;

pub struct Reconciler {
    pool: SqlitePool,
    precedence: Precedence,
    primary: Option<String>,
    bus: EventBus,
}

impl Reconciler {
    /// `primary` is the only source that creates canonical entities; `None`
    /// means nothing is created automatically.
    pub fn new(
        pool: SqlitePool,
        precedence: Precedence,
        primary: Option<String>,
        bus: EventBus,
    ) -> Self {
        Self {
            pool,
            precedence,
            primary,
            bus,
        }
    }

    pub fn bus(&self) -> &EventBus {
        &self.bus
    }

    pub fn pool(&self) -> &SqlitePool {
        &self.pool
    }

    fn is_primary(&self, source: &str) -> bool {
        self.primary.as_deref() == Some(source)
    }

    /// Runs `f` in one transaction; commits, then publishes the events.
    async fn in_tx<T>(
        &self,
        f: impl AsyncFnOnce(&mut Conn, &mut Report, &mut Events) -> Result<T>,
    ) -> Result<(T, Report)> {
        let mut rep = Report::default();
        let mut out = Events::new();
        let mut tx = store::begin_write(&self.pool).await?;
        let v = f(&mut tx, &mut rep, &mut out).await?;
        tx.commit().await?;
        rep.events += u32::try_from(out.len()).unwrap_or(u32::MAX);
        self.bus.publish(out);
        Ok((v, rep))
    }

    /// Like [`Self::in_tx`] for one item of a batch: a failure is logged and
    /// counted instead of aborting the batch.
    async fn batch_item(
        &self,
        report: &mut Report,
        what: &str,
        f: impl AsyncFnOnce(&mut Conn, &mut Report, &mut Events) -> Result<()>,
    ) {
        match self.in_tx(f).await {
            Ok(((), rep)) => *report += rep,
            Err(e) => {
                tracing::warn!("cannot reconcile {what}: {e}");
                report.failed += 1;
            }
        }
    }

    // ---- entry points -----------------------------------------------------

    /// Stores and reconciles one event with its stages.
    pub async fn ingest_event(
        &self,
        source: &str,
        ev: &Sourced<SrcEvent>,
        now: DateTime<Utc>,
    ) -> Result<Report> {
        let may_create = self.is_primary(source);
        let ((), mut report) = self
            .in_tx(async |c, rep, _| {
                record::store(
                    c,
                    source,
                    EntityType::Event,
                    &ev.dto.ext_id,
                    &ev.dto,
                    &ev.raw,
                    now,
                )
                .await?;
                self.reconcile_event(c, source, &ev.dto, may_create, now, rep)
                    .await?;
                Ok(())
            })
            .await?;
        self.retry_queue(source, now, &mut report).await?;
        Ok(report)
    }

    /// Stores and reconciles a batch of matches (with their teams), one
    /// transaction per match. Bracket feeds that point to a match later in
    /// the batch are resolved in a second pass.
    pub async fn ingest_matches(
        &self,
        source: &str,
        batch: &[Sourced<SrcMatch>],
        now: DateTime<Utc>,
    ) -> Result<Report> {
        let may_create = self.is_primary(source);
        let mut report = Report::default();
        for s in batch {
            let what = format!("{source} match {}", s.dto.ext_id);
            self.batch_item(&mut report, &what, async |c, rep, out| {
                for t in [&s.dto.team_a, &s.dto.team_b].into_iter().flatten() {
                    record::store(c, source, EntityType::Team, &t.ext_id, t, &Value::Null, now)
                        .await?;
                }
                record::store(
                    c,
                    source,
                    EntityType::Match,
                    &s.dto.ext_id,
                    &s.dto,
                    &s.raw,
                    now,
                )
                .await?;
                self.reconcile_match(c, source, &s.dto, may_create, now, rep, out)
                    .await?;
                Ok(())
            })
            .await;
        }
        let with_feeds: Vec<&SrcMatch> = batch.iter().map(|s| &s.dto).collect();
        self.resolve_feeds(source, &with_feeds, now, &mut report)
            .await;
        self.retry_queue(source, now, &mut report).await?;
        Ok(report)
    }

    /// Re-merges one match from the stored records, e.g. after T05 changed
    /// a manual override.
    pub async fn refresh_match(&self, id: MatchId, now: DateTime<Utc>) -> Result<Report> {
        let ((), rep) = self
            .in_tx(async |c, rep, out| self.refresh_match_in(c, id, now, rep, out).await)
            .await?;
        Ok(rep)
    }

    /// Links an external ID to an existing canonical entity (admin action,
    /// T08) and re-merges it. If the ID was linked elsewhere before, that
    /// entity is re-merged too. Resolves the queue entry and retries the
    /// source's other queued entries, which may depend on this one.
    pub async fn link(
        &self,
        source: &str,
        entity_type: EntityType,
        external_id: &str,
        entity_id: i64,
        actor: &str,
        now: DateTime<Utc>,
    ) -> Result<Report> {
        let ((), mut report) = self
            .in_tx(async |c, rep, out| {
                if !exists(c, entity_type, entity_id).await? {
                    return Err(ReconcileError::Invalid(format!(
                        "{entity_type} {entity_id} does not exist"
                    )));
                }
                let previous = refs::resolve(&mut *c, source, entity_type, external_id).await?;
                refs::relink(
                    &mut *c,
                    &ExternalRef {
                        entity_type,
                        entity_id,
                        source: source.to_owned(),
                        external_id: external_id.to_owned(),
                    },
                )
                .await?;
                unlinked::remove(&mut *c, source, entity_type, external_id).await?;
                audit::append(
                    &mut *c,
                    &AuditRecord {
                        actor: actor.to_owned(),
                        action: "link_external_id".to_owned(),
                        entity_type: entity_type.as_str().to_owned(),
                        entity_id: Some(entity_id.to_string()),
                        before: previous.map(|p| json!({ "entity_id": p })),
                        after: Some(json!({ "source": source, "external_id": external_id })),
                        at: now,
                    },
                )
                .await?;
                rep.linked += 1;
                self.refresh_any(
                    c,
                    source,
                    entity_type,
                    external_id,
                    entity_id,
                    now,
                    rep,
                    out,
                )
                .await?;
                if let Some(p) = previous.filter(|&p| p != entity_id) {
                    self.refresh_any(c, source, entity_type, external_id, p, now, rep, out)
                        .await?;
                }
                Ok(())
            })
            .await?;
        self.retry_queue(source, now, &mut report).await?;
        Ok(report)
    }

    /// Creates a new canonical entity from a queued external entity (admin
    /// action, T08) and returns its ID. A match's event and teams must be
    /// linked first; an event's stages are created with it.
    pub async fn create_from_unlinked(
        &self,
        source: &str,
        entity_type: EntityType,
        external_id: &str,
        actor: &str,
        now: DateTime<Utc>,
    ) -> Result<(i64, Report)> {
        let missing = || {
            ReconcileError::Invalid(format!(
                "no stored record for {source} {entity_type} {external_id}"
            ))
        };
        let (id, mut report) = self
            .in_tx(async |c, rep, out| {
                if refs::resolve(&mut *c, source, entity_type, external_id).await?.is_some() {
                    return Err(ReconcileError::Invalid(format!(
                        "{source} {entity_type} {external_id} is already linked"
                    )));
                }
                let id = match entity_type {
                    EntityType::Team => {
                        let dto: SrcTeam = record::load(c, source, entity_type, external_id)
                            .await?
                            .ok_or_else(missing)?;
                        self.reconcile_team(c, source, &dto, true, now, rep).await?.map(|t| t.0)
                    }
                    EntityType::Event => {
                        let dto: SrcEvent = record::load(c, source, entity_type, external_id)
                            .await?
                            .ok_or_else(missing)?;
                        self.reconcile_event(c, source, &dto, true, now, rep).await?.map(|e| e.0)
                    }
                    EntityType::Stage => {
                        let (ev, st) = find_stage_record(c, source, external_id)
                            .await?
                            .ok_or_else(missing)?;
                        match refs::resolve(&mut *c, source, EntityType::Event, &ev.ext_id).await? {
                            Some(e) => self
                                .reconcile_stage(c, source, EventId(e), &st, true, now, rep)
                                .await?
                                .map(|s| s.0),
                            None => None,
                        }
                    }
                    EntityType::Match => {
                        let dto: SrcMatch = record::load(c, source, entity_type, external_id)
                            .await?
                            .ok_or_else(missing)?;
                        self.reconcile_match(c, source, &dto, true, now, rep, out)
                            .await?
                            .map(|m| m.0)
                    }
                };
                let id = id.ok_or_else(|| {
                    ReconcileError::Invalid(format!(
                        "{source} {entity_type} {external_id} depends on unlinked entities; link them first"
                    ))
                })?;
                audit::append(
                    &mut *c,
                    &AuditRecord {
                        actor: actor.to_owned(),
                        action: "create_from_unlinked".to_owned(),
                        entity_type: entity_type.as_str().to_owned(),
                        entity_id: Some(id.to_string()),
                        before: None,
                        after: Some(json!({ "source": source, "external_id": external_id })),
                        at: now,
                    },
                )
                .await?;
                Ok(id)
            })
            .await?;
        self.retry_queue(source, now, &mut report).await?;
        Ok((id, report))
    }

    /// Rebuilds canonical data from `source_records` without API calls: the
    /// primary source first, then the others in precedence order; per
    /// source events, then teams, then matches. Idempotent; on an empty
    /// database it recreates everything the records describe. Manual
    /// overrides apply only where their links still exist.
    pub async fn replay(&self, now: DateTime<Utc>) -> Result<Report> {
        let mut sources: Vec<String> = self.primary.iter().cloned().collect();
        for s in self.precedence.sources() {
            if !sources.contains(&s) {
                sources.push(s);
            }
        }
        let mut report = Report::default();
        for source in &sources {
            let source = source.as_str();
            let may_create = self.is_primary(source);
            for rec in refs::list_records(&self.pool, source, EntityType::Event).await? {
                let what = format!("{source} event {}", rec.external_id);
                self.batch_item(&mut report, &what, async |c, rep, _| {
                    let dto: SrcEvent = record::decode(&rec)?;
                    self.reconcile_event(c, source, &dto, may_create, now, rep)
                        .await?;
                    Ok(())
                })
                .await;
            }
            for rec in refs::list_records(&self.pool, source, EntityType::Team).await? {
                let what = format!("{source} team {}", rec.external_id);
                self.batch_item(&mut report, &what, async |c, rep, _| {
                    let dto: SrcTeam = record::decode(&rec)?;
                    self.reconcile_team(c, source, &dto, may_create, now, rep)
                        .await?;
                    Ok(())
                })
                .await;
            }
            let mut dtos = Vec::new();
            for rec in refs::list_records(&self.pool, source, EntityType::Match).await? {
                let what = format!("{source} match {}", rec.external_id);
                let dto: SrcMatch = match record::decode(&rec) {
                    Ok(d) => d,
                    Err(e) => {
                        tracing::warn!("cannot reconcile {what}: {e}");
                        report.failed += 1;
                        continue;
                    }
                };
                self.batch_item(&mut report, &what, async |c, rep, out| {
                    self.reconcile_match(c, source, &dto, may_create, now, rep, out)
                        .await?;
                    Ok(())
                })
                .await;
                dtos.push(dto);
            }
            let refs: Vec<&SrcMatch> = dtos.iter().collect();
            self.resolve_feeds(source, &refs, now, &mut report).await;
        }
        Ok(report)
    }

    // ---- internals --------------------------------------------------------

    /// Second pass for matches with bracket feeds: re-merge them now that
    /// the matches they point to may exist.
    async fn resolve_feeds(
        &self,
        source: &str,
        dtos: &[&SrcMatch],
        now: DateTime<Utc>,
        report: &mut Report,
    ) {
        for dto in dtos {
            let has_feeds = dto
                .bracket
                .as_ref()
                .is_some_and(|b| b.feeds_a.is_some() || b.feeds_b.is_some());
            if !has_feeds {
                continue;
            }
            let what = format!("feeds of {source} match {}", dto.ext_id);
            self.batch_item(report, &what, async |c, rep, out| {
                if let Some(id) =
                    refs::resolve(&mut *c, source, EntityType::Match, &dto.ext_id).await?
                {
                    self.refresh_match_in(c, MatchId(id), now, rep, out).await?;
                }
                Ok(())
            })
            .await;
        }
    }

    /// Retries the source's queued entries; resolved ones leave the queue.
    async fn retry_queue(
        &self,
        source: &str,
        now: DateTime<Utc>,
        report: &mut Report,
    ) -> Result<()> {
        let mut queued = unlinked::list(&self.pool, Some(source)).await?;
        let order = |t: EntityType| match t {
            EntityType::Event => 0,
            EntityType::Stage => 1,
            EntityType::Team => 2,
            EntityType::Match => 3,
        };
        queued.sort_by_key(|q| order(q.entity_type));
        let may_create = self.is_primary(source);
        let mut retried = Report::default();
        for q in queued {
            let what = format!("queued {source} {} {}", q.entity_type, q.external_id);
            let ext = q.external_id.as_str();
            self.batch_item(&mut retried, &what, async |c, rep, out| {
                let gone = match q.entity_type {
                    EntityType::Event => {
                        match record::load::<SrcEvent>(c, source, q.entity_type, ext).await? {
                            Some(dto) => {
                                self.reconcile_event(c, source, &dto, may_create, now, rep)
                                    .await?;
                                false
                            }
                            None => true,
                        }
                    }
                    EntityType::Stage => match find_stage_record(c, source, ext).await? {
                        Some((ev, st)) => {
                            if let Some(e) =
                                refs::resolve(&mut *c, source, EntityType::Event, &ev.ext_id)
                                    .await?
                            {
                                self.reconcile_stage(
                                    c,
                                    source,
                                    EventId(e),
                                    &st,
                                    may_create,
                                    now,
                                    rep,
                                )
                                .await?;
                            }
                            false
                        }
                        None => true,
                    },
                    EntityType::Team => {
                        match record::load::<SrcTeam>(c, source, q.entity_type, ext).await? {
                            Some(dto) => {
                                self.reconcile_team(c, source, &dto, may_create, now, rep)
                                    .await?;
                                false
                            }
                            None => true,
                        }
                    }
                    EntityType::Match => {
                        match record::load::<SrcMatch>(c, source, q.entity_type, ext).await? {
                            Some(dto) => {
                                self.reconcile_match(c, source, &dto, may_create, now, rep, out)
                                    .await?;
                                false
                            }
                            None => true,
                        }
                    }
                };
                if gone {
                    unlinked::remove(&mut *c, source, q.entity_type, ext).await?;
                }
                Ok(())
            })
            .await;
        }
        // Entries that stay queued were already counted when first queued.
        retried.queued = 0;
        *report += retried;
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    async fn queue(
        &self,
        c: &mut Conn,
        source: &str,
        entity_type: EntityType,
        external_id: &str,
        reason: UnlinkedReason,
        detail: String,
        now: DateTime<Utc>,
        rep: &mut Report,
    ) -> Result<()> {
        tracing::debug!(source, %entity_type, external_id, %reason, detail, "unlinked");
        unlinked::upsert(
            c,
            &UnlinkedRef {
                source: source.to_owned(),
                entity_type,
                external_id: external_id.to_owned(),
                reason,
                detail: Some(detail),
                first_seen_at: now,
            },
        )
        .await?;
        rep.queued += 1;
        Ok(())
    }

    /// Records a new link from an identity rule or a creation.
    async fn add_link(
        &self,
        c: &mut Conn,
        source: &str,
        entity_type: EntityType,
        external_id: &str,
        entity_id: i64,
    ) -> Result<()> {
        refs::link(
            &mut *c,
            &ExternalRef {
                entity_type,
                entity_id,
                source: source.to_owned(),
                external_id: external_id.to_owned(),
            },
        )
        .await?;
        unlinked::remove(&mut *c, source, entity_type, external_id).await?;
        Ok(())
    }

    /// Re-merges any entity after a link change.
    #[allow(clippy::too_many_arguments)]
    async fn refresh_any(
        &self,
        c: &mut Conn,
        source: &str,
        entity_type: EntityType,
        external_id: &str,
        id: i64,
        now: DateTime<Utc>,
        rep: &mut Report,
        out: &mut Events,
    ) -> Result<()> {
        match entity_type {
            EntityType::Team => self.refresh_team(c, TeamId(id), now, rep).await,
            EntityType::Stage => self.refresh_stage(c, StageId(id), now, rep).await,
            EntityType::Match => self.refresh_match_in(c, MatchId(id), now, rep, out).await,
            EntityType::Event => {
                self.refresh_event(c, EventId(id), now, rep).await?;
                // Link or create the newly linked event's stages too.
                if let Some(dto) =
                    record::load::<SrcEvent>(c, source, entity_type, external_id).await?
                {
                    if refs::resolve(&mut *c, source, entity_type, external_id).await? == Some(id) {
                        let may_create = self.is_primary(source);
                        for st in &dto.stages {
                            self.reconcile_stage(c, source, EventId(id), st, may_create, now, rep)
                                .await?;
                        }
                    }
                }
                Ok(())
            }
        }
    }

    // ---- events and stages ------------------------------------------------

    async fn reconcile_event(
        &self,
        c: &mut Conn,
        source: &str,
        dto: &SrcEvent,
        may_create: bool,
        now: DateTime<Utc>,
        rep: &mut Report,
    ) -> Result<Option<EventId>> {
        let id = match refs::resolve(&mut *c, source, EntityType::Event, &dto.ext_id).await? {
            Some(id) => {
                let id = EventId(id);
                self.refresh_event(c, id, now, rep).await?;
                id
            }
            None if source == MANUAL => return Ok(None),
            None if may_create => {
                let (new, from) =
                    merge::merge_event(&[(source.to_owned(), dto.clone())], &self.precedence)
                        .ok_or_else(|| not_in_precedence(source, FieldGroup::Event))?;
                let id = events::insert(&mut *c, &new).await?;
                self.add_link(c, source, EntityType::Event, &dto.ext_id, id.0)
                    .await?;
                provenance::set(
                    &mut *c,
                    EntityType::Event,
                    id.0,
                    FieldGroup::Event.as_str(),
                    &from,
                    now,
                )
                .await?;
                rep.created += 1;
                id
            }
            None => {
                self.queue(
                    c,
                    source,
                    EntityType::Event,
                    &dto.ext_id,
                    UnlinkedReason::NoMatch,
                    format!(
                        "event \"{}\": events of non-primary sources need a manual link",
                        dto.name
                    ),
                    now,
                    rep,
                )
                .await?;
                return Ok(None);
            }
        };
        for st in &dto.stages {
            self.reconcile_stage(c, source, id, st, may_create, now, rep)
                .await?;
        }
        Ok(Some(id))
    }

    async fn refresh_event(
        &self,
        c: &mut Conn,
        id: EventId,
        now: DateTime<Utc>,
        rep: &mut Report,
    ) -> Result<()> {
        let cur = events::get(&mut *c, id)
            .await?
            .ok_or_else(|| gone(EntityType::Event, id.0))?;
        let mut views = Vec::new();
        for r in refs::refs_of(&mut *c, EntityType::Event, id.0).await? {
            if let Some(dto) =
                record::load::<SrcEvent>(c, &r.source, EntityType::Event, &r.external_id).await?
            {
                views.push((r.source, dto));
            }
        }
        let Some((new, from)) = merge::merge_event(&views, &self.precedence) else {
            return Ok(());
        };
        let next = Event {
            id,
            name: new.name,
            tier: new.tier,
            begin_at: new.begin_at,
            end_at: new.end_at,
            status: new.status,
        };
        if next != cur {
            events::update(&mut *c, &next).await?;
            rep.updated += 1;
        }
        provenance::set(
            &mut *c,
            EntityType::Event,
            id.0,
            FieldGroup::Event.as_str(),
            &from,
            now,
        )
        .await?;
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    async fn reconcile_stage(
        &self,
        c: &mut Conn,
        source: &str,
        event_id: EventId,
        dto: &SrcStage,
        may_create: bool,
        now: DateTime<Utc>,
        rep: &mut Report,
    ) -> Result<Option<StageId>> {
        let id = match refs::resolve(&mut *c, source, EntityType::Stage, &dto.ext_id).await? {
            Some(id) => StageId(id),
            None if source == MANUAL => return Ok(None),
            None if may_create => {
                let (name, kind, position, from) =
                    merge::merge_stage(&[(source.to_owned(), dto.clone())], &self.precedence)
                        .ok_or_else(|| not_in_precedence(source, FieldGroup::Stage))?;
                let id = events::insert_stage(
                    &mut *c,
                    &NewStage {
                        event_id,
                        name,
                        kind,
                        position,
                    },
                )
                .await?;
                self.add_link(c, source, EntityType::Stage, &dto.ext_id, id.0)
                    .await?;
                provenance::set(
                    &mut *c,
                    EntityType::Stage,
                    id.0,
                    FieldGroup::Stage.as_str(),
                    &from,
                    now,
                )
                .await?;
                rep.created += 1;
                return Ok(Some(id));
            }
            None => {
                let stages = events::list_stages(&mut *c, event_id).await?;
                let cands: Vec<i64> = identity::stage_candidates(&stages, dto)
                    .iter()
                    .map(|s| s.id.0)
                    .collect();
                match self
                    .pick_candidate(
                        c,
                        source,
                        EntityType::Stage,
                        &dto.ext_id,
                        &cands,
                        &format!("stage \"{}\"", dto.name),
                        now,
                        rep,
                    )
                    .await?
                {
                    Some(id) => StageId(id),
                    None => return Ok(None),
                }
            }
        };
        self.refresh_stage(c, id, now, rep).await?;
        Ok(Some(id))
    }

    async fn refresh_stage(
        &self,
        c: &mut Conn,
        id: StageId,
        now: DateTime<Utc>,
        rep: &mut Report,
    ) -> Result<()> {
        let cur = events::get_stage(&mut *c, id)
            .await?
            .ok_or_else(|| gone(EntityType::Stage, id.0))?;
        // A stage's DTO lives inside its source's event record.
        let event_refs = refs::refs_of(&mut *c, EntityType::Event, cur.event_id.0).await?;
        let mut views = Vec::new();
        for r in refs::refs_of(&mut *c, EntityType::Stage, id.0).await? {
            for er in event_refs.iter().filter(|er| er.source == r.source) {
                if let Some(ev) =
                    record::load::<SrcEvent>(c, &er.source, EntityType::Event, &er.external_id)
                        .await?
                {
                    if let Some(st) = ev.stages.into_iter().find(|s| s.ext_id == r.external_id) {
                        views.push((r.source.clone(), st));
                    }
                }
            }
        }
        let Some((name, kind, position, from)) = merge::merge_stage(&views, &self.precedence)
        else {
            return Ok(());
        };
        let next = Stage {
            id,
            event_id: cur.event_id,
            name,
            kind,
            position,
        };
        if next != cur {
            events::update_stage(&mut *c, &next).await?;
            rep.updated += 1;
        }
        provenance::set(
            &mut *c,
            EntityType::Stage,
            id.0,
            FieldGroup::Stage.as_str(),
            &from,
            now,
        )
        .await?;
        Ok(())
    }

    // ---- teams ------------------------------------------------------------

    async fn reconcile_team(
        &self,
        c: &mut Conn,
        source: &str,
        dto: &SrcTeam,
        may_create: bool,
        now: DateTime<Utc>,
        rep: &mut Report,
    ) -> Result<Option<TeamId>> {
        let id = match refs::resolve(&mut *c, source, EntityType::Team, &dto.ext_id).await? {
            Some(id) => TeamId(id),
            None if source == MANUAL => return Ok(None),
            None if may_create => {
                let (new, from) =
                    merge::merge_team(&[(source.to_owned(), dto.clone())], &self.precedence)
                        .ok_or_else(|| not_in_precedence(source, FieldGroup::Team))?;
                let id = teams::insert(&mut *c, &new).await?;
                self.add_link(c, source, EntityType::Team, &dto.ext_id, id.0)
                    .await?;
                provenance::set(
                    &mut *c,
                    EntityType::Team,
                    id.0,
                    FieldGroup::Team.as_str(),
                    &from,
                    now,
                )
                .await?;
                rep.created += 1;
                return Ok(Some(id));
            }
            None => {
                let all = teams::list(&mut *c).await?;
                let cands: Vec<i64> = identity::team_candidates(&all, dto)
                    .iter()
                    .map(|t| t.id.0)
                    .collect();
                match self
                    .pick_candidate(
                        c,
                        source,
                        EntityType::Team,
                        &dto.ext_id,
                        &cands,
                        &format!("team \"{}\"", dto.name),
                        now,
                        rep,
                    )
                    .await?
                {
                    Some(id) => TeamId(id),
                    None => return Ok(None),
                }
            }
        };
        self.refresh_team(c, id, now, rep).await?;
        Ok(Some(id))
    }

    async fn refresh_team(
        &self,
        c: &mut Conn,
        id: TeamId,
        now: DateTime<Utc>,
        rep: &mut Report,
    ) -> Result<()> {
        let cur = teams::get(&mut *c, id)
            .await?
            .ok_or_else(|| gone(EntityType::Team, id.0))?;
        let mut views = Vec::new();
        for r in refs::refs_of(&mut *c, EntityType::Team, id.0).await? {
            if let Some(dto) =
                record::load::<SrcTeam>(c, &r.source, EntityType::Team, &r.external_id).await?
            {
                views.push((r.source, dto));
            }
        }
        let Some((new, from)) = merge::merge_team(&views, &self.precedence) else {
            return Ok(());
        };
        let next = Team {
            id,
            name: new.name,
            acronym: new.acronym,
            logo_url: new.logo_url,
            country: new.country,
        };
        if next != cur {
            teams::update(&mut *c, &next).await?;
            rep.updated += 1;
        }
        provenance::set(
            &mut *c,
            EntityType::Team,
            id.0,
            FieldGroup::Team.as_str(),
            &from,
            now,
        )
        .await?;
        Ok(())
    }

    /// Links to the single candidate, or queues as no match / ambiguous.
    /// Ambiguity is decided on all candidates, so linking one of two
    /// look-alikes never makes the other an automatic match.
    #[allow(clippy::too_many_arguments)]
    async fn pick_candidate(
        &self,
        c: &mut Conn,
        source: &str,
        entity_type: EntityType,
        external_id: &str,
        cands: &[i64],
        label: &str,
        now: DateTime<Utc>,
        rep: &mut Report,
    ) -> Result<Option<i64>> {
        match cands {
            // The source already maps another of its IDs to this entity, so
            // this one is something else (or a source-side duplicate).
            [one] if linked_by(c, entity_type, *one, source).await? => {
                self.queue(
                    c,
                    source,
                    entity_type,
                    external_id,
                    UnlinkedReason::NoMatch,
                    format!("{label}: the only candidate, {entity_type} {one}, is already linked to another {source} ID"),
                    now,
                    rep,
                )
                .await?;
                Ok(None)
            }
            [one] => {
                let id = *one;
                self.add_link(c, source, entity_type, external_id, id)
                    .await?;
                rep.linked += 1;
                Ok(Some(id))
            }
            [] => {
                self.queue(
                    c,
                    source,
                    entity_type,
                    external_id,
                    UnlinkedReason::NoMatch,
                    format!("{label}: no matching {entity_type}"),
                    now,
                    rep,
                )
                .await?;
                Ok(None)
            }
            many => {
                let ids: Vec<String> = many.iter().map(|i| i.to_string()).collect();
                self.queue(
                    c,
                    source,
                    entity_type,
                    external_id,
                    UnlinkedReason::Ambiguous,
                    format!("{label}: candidates {entity_type} {}", ids.join(", ")),
                    now,
                    rep,
                )
                .await?;
                Ok(None)
            }
        }
    }

    // ---- matches ----------------------------------------------------------

    /// Resolves (or creates, or queues) a match and re-merges it. Returns
    /// the canonical ID, or `None` if it is queued.
    #[allow(clippy::too_many_arguments)]
    async fn reconcile_match(
        &self,
        c: &mut Conn,
        source: &str,
        dto: &SrcMatch,
        may_create: bool,
        now: DateTime<Utc>,
        rep: &mut Report,
        out: &mut Events,
    ) -> Result<Option<MatchId>> {
        let existing = refs::resolve(&mut *c, source, EntityType::Match, &dto.ext_id).await?;
        if existing.is_none() && source == MANUAL {
            return Ok(None);
        }
        // Teams first: the primary creates them, others link or queue them.
        // Only the match's own creation may be forced by an admin, so teams
        // follow the primary rule.
        let teams_may_create = self.is_primary(source);
        let mut team_ids = [None, None];
        let mut missing = Vec::new();
        for (slot, t) in [&dto.team_a, &dto.team_b].into_iter().enumerate() {
            if let Some(t) = t {
                match self
                    .reconcile_team(c, source, t, teams_may_create, now, rep)
                    .await?
                {
                    Some(id) => team_ids[slot] = Some(id),
                    None => missing.push(format!("team {} \"{}\"", t.ext_id, t.name)),
                }
            }
        }
        let id = match existing {
            Some(id) => MatchId(id),
            None => {
                let queue_dep = async |c: &mut Conn, rep: &mut Report, what: String| {
                    self.queue(
                        c,
                        source,
                        EntityType::Match,
                        &dto.ext_id,
                        UnlinkedReason::Dependency,
                        what,
                        now,
                        rep,
                    )
                    .await
                };
                let Some(event_id) =
                    refs::resolve(&mut *c, source, EntityType::Event, &dto.event_ext_id).await?
                else {
                    queue_dep(c, rep, format!("event {} is not linked", dto.event_ext_id)).await?;
                    return Ok(None);
                };
                let event_id = EventId(event_id);
                if !missing.is_empty() {
                    queue_dep(c, rep, format!("{} not linked", missing.join(" and "))).await?;
                    return Ok(None);
                }
                if may_create {
                    return self
                        .create_match(c, source, dto, event_id, now, rep, out)
                        .await
                        .map(Some);
                }
                // Rule 3: same event, same two teams, time within ±2 h.
                let (Some(a), Some(b), Some(at)) = (team_ids[0], team_ids[1], dto.scheduled_at)
                else {
                    self.queue(
                        c,
                        source,
                        EntityType::Match,
                        &dto.ext_id,
                        UnlinkedReason::NoMatch,
                        "needs both teams and a scheduled time to match automatically".to_owned(),
                        now,
                        rep,
                    )
                    .await?;
                    return Ok(None);
                };
                let (from, to) = identity::match_window(at);
                let cands: Vec<i64> = matches::find_between(&mut *c, event_id, a, b, from, to)
                    .await?
                    .iter()
                    .map(|m| m.id.0)
                    .collect();
                match self
                    .pick_candidate(
                        c,
                        source,
                        EntityType::Match,
                        &dto.ext_id,
                        &cands,
                        "match",
                        now,
                        rep,
                    )
                    .await?
                {
                    Some(id) => MatchId(id),
                    None => return Ok(None),
                }
            }
        };
        self.refresh_match_in(c, id, now, rep, out).await?;
        Ok(Some(id))
    }

    #[allow(clippy::too_many_arguments)]
    async fn create_match(
        &self,
        c: &mut Conn,
        source: &str,
        dto: &SrcMatch,
        event_id: EventId,
        now: DateTime<Utc>,
        rep: &mut Report,
        out: &mut Events,
    ) -> Result<MatchId> {
        let view = self.build_view(c, source, dto.clone()).await?;
        if self.precedence.sources().iter().all(|s| s != source) {
            return Err(not_in_precedence(source, FieldGroup::Teams));
        }
        let merged = merge::merge_match(MatchId(0), event_id, None, &[view], &self.precedence);
        let id = matches::insert(&mut *c, &merged.fields).await?;
        self.add_link(c, source, EntityType::Match, &dto.ext_id, id.0)
            .await?;
        for mut map in merged.maps.clone() {
            map.match_id = id;
            matches::upsert_map(&mut *c, &map).await?;
        }
        write_provenance(c, EntityType::Match, id.0, &merged, now).await?;
        rep.created += 1;
        out.push(DomainEvent::MatchCreated { match_id: id });
        Ok(id)
    }

    pub(crate) async fn refresh_match_in(
        &self,
        c: &mut Conn,
        id: MatchId,
        now: DateTime<Utc>,
        rep: &mut Report,
        out: &mut Events,
    ) -> Result<()> {
        let cur = matches::get(&mut *c, id)
            .await?
            .ok_or_else(|| gone(EntityType::Match, id.0))?;
        let cur_maps = matches::list_maps(&mut *c, id).await?;
        let mut views = Vec::new();
        for r in refs::refs_of(&mut *c, EntityType::Match, id.0).await? {
            if let Some(dto) =
                record::load::<SrcMatch>(c, &r.source, EntityType::Match, &r.external_id).await?
            {
                views.push(self.build_view(c, &r.source, dto).await?);
            }
        }
        let merged = merge::merge_match(
            id,
            cur.fields.event_id,
            Some((&cur.fields, &cur_maps)),
            &views,
            &self.precedence,
        );
        let new = Match {
            id,
            fields: merged.fields.clone(),
        };
        let mut changed = false;
        if new.fields != cur.fields {
            matches::update(&mut *c, &new).await?;
            changed = true;
        }
        for m in &merged.maps {
            if cur_maps.iter().find(|cm| cm.number == m.number) != Some(m) {
                matches::upsert_map(&mut *c, m).await?;
                changed = true;
            }
        }
        write_provenance(c, EntityType::Match, id.0, &merged, now).await?;
        if changed {
            rep.updated += 1;
        }
        out.extend(diff::match_events(&cur, &cur_maps, &new, &merged.maps));
        Ok(())
    }

    /// Resolves a DTO's external references to canonical IDs.
    async fn build_view(&self, c: &mut Conn, source: &str, dto: SrcMatch) -> Result<MatchView> {
        let mut team = [None, None];
        for (slot, t) in [&dto.team_a, &dto.team_b].into_iter().enumerate() {
            if let Some(t) = t {
                team[slot] = refs::resolve(&mut *c, source, EntityType::Team, &t.ext_id)
                    .await?
                    .map(TeamId);
            }
        }
        let stage = match &dto.stage_ext_id {
            Some(ext) => refs::resolve(&mut *c, source, EntityType::Stage, ext)
                .await?
                .map(StageId),
            None => None,
        };
        let bracket = dto.bracket.clone().unwrap_or_default();
        let mut feeds = [None, None];
        for (slot, f) in [&bracket.feeds_a, &bracket.feeds_b].into_iter().enumerate() {
            if let Some(f) = f {
                feeds[slot] = refs::resolve(&mut *c, source, EntityType::Match, &f.match_ext_id)
                    .await?
                    .map(|id| Feed {
                        match_id: MatchId(id),
                        outcome: f.outcome,
                    });
            }
        }
        Ok(MatchView {
            source: source.to_owned(),
            dto,
            team_a: team[0],
            team_b: team[1],
            stage,
            feeds_a: feeds[0],
            feeds_b: feeds[1],
        })
    }
}

// ---- free helpers -----------------------------------------------------------

fn gone(entity_type: EntityType, id: i64) -> ReconcileError {
    ReconcileError::Invalid(format!("{entity_type} {id} does not exist"))
}

fn not_in_precedence(source: &str, group: FieldGroup) -> ReconcileError {
    ReconcileError::Invalid(format!(
        "source {source} takes no part in the {group} precedence; is it enabled?"
    ))
}

/// Whether `source` already links some external ID to this entity. Such an
/// entity is no candidate for another external ID of the same source.
async fn linked_by(c: &mut Conn, entity_type: EntityType, id: i64, source: &str) -> Result<bool> {
    Ok(refs::refs_of(&mut *c, entity_type, id)
        .await?
        .iter()
        .any(|r| r.source == source))
}

async fn exists(c: &mut Conn, entity_type: EntityType, id: i64) -> Result<bool> {
    Ok(match entity_type {
        EntityType::Team => teams::get(&mut *c, TeamId(id)).await?.is_some(),
        EntityType::Event => events::get(&mut *c, EventId(id)).await?.is_some(),
        EntityType::Stage => events::get_stage(&mut *c, StageId(id)).await?.is_some(),
        EntityType::Match => matches::get(&mut *c, MatchId(id)).await?.is_some(),
    })
}

/// Finds the event record of `source` that contains a stage.
async fn find_stage_record(
    c: &mut Conn,
    source: &str,
    stage_ext_id: &str,
) -> Result<Option<(SrcEvent, SrcStage)>> {
    for rec in refs::list_records(&mut *c, source, EntityType::Event).await? {
        let ev: SrcEvent = record::decode(&rec)?;
        if let Some(st) = ev.stages.iter().find(|s| s.ext_id == stage_ext_id).cloned() {
            return Ok(Some((ev, st)));
        }
    }
    Ok(None)
}

async fn write_provenance(
    c: &mut Conn,
    entity_type: EntityType,
    id: i64,
    merged: &MergedMatch,
    now: DateTime<Utc>,
) -> Result<()> {
    for (field, source) in &merged.provenance {
        provenance::set(&mut *c, entity_type, id, field, source, now).await?;
    }
    Ok(())
}
