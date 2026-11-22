//! Following and unfollowing events per guild (T09). Only followed events
//! are synced and pickable.
//!
//! - [`follow_hit`] follows an event from a search result
//!   ([`SyncHandle::search`]): the result is ingested like any source data
//!   (event and stages, no matches), so the event gets its canonical ID and
//!   its link to the source, then [`follow`] runs.
//! - [`follow`] follows an event that is already in the database, writes
//!   the audit log and asks the scheduler for an immediate structure sync,
//!   which brings the matches.
//! - [`unfollow`] stops syncing for this guild and hides the event's
//!   matches. Picks, results and history stay as they are.
//!
//! Only the primary source creates canonical events. A search result of
//! another source that is not linked yet goes to the unlinked queue, and
//! following it fails with [`FollowError::NeedsLink`].

use chrono::{DateTime, Utc};
use domain::{EntityType, Event, EventId};
use serde_json::json;
use store::SqlitePool;
use store::audit::AuditRecord;
use store::guilds::FollowedEvent;

use crate::handle::{SearchHit, SyncHandle};
use crate::reconcile::{ReconcileError, Reconciler};

#[derive(Debug, thiserror::Error)]
pub enum FollowError {
    #[error("database error: {0}")]
    Db(#[from] sqlx::Error),
    #[error(transparent)]
    Reconcile(#[from] ReconcileError),
    #[error("event {0} does not exist")]
    UnknownEvent(EventId),
    /// The result came from a source that may not create events, and no
    /// admin linked it to a canonical event yet.
    #[error("event \"{name}\" from {source_id} is not linked to a known event yet")]
    NeedsLink { source_id: String, name: String },
}

/// What [`follow`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FollowOutcome {
    pub event: Event,
    /// `false` if the guild followed it already; nothing changed then.
    pub newly_followed: bool,
}

/// What [`unfollow`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnfollowOutcome {
    pub event: Event,
    /// `false` if the guild did not follow it; nothing changed then.
    pub was_followed: bool,
}

/// Ingests a search result, then follows its event (see [`follow`]).
pub async fn follow_hit(
    reconciler: &Reconciler,
    sync: Option<&SyncHandle>,
    guild_id: &str,
    admin_id: &str,
    hit: &SearchHit,
    now: DateTime<Utc>,
) -> Result<FollowOutcome, FollowError> {
    reconciler.ingest_event(hit.source, &hit.event, now).await?;
    let pool = reconciler.pool();
    let id = store::refs::resolve(pool, hit.source, EntityType::Event, &hit.event.dto.ext_id)
        .await?
        .map(EventId)
        .ok_or_else(|| FollowError::NeedsLink {
            source_id: hit.source.to_owned(),
            name: hit.event.dto.name.clone(),
        })?;
    follow(pool, sync, guild_id, admin_id, id, now).await
}

/// Follows a known event in a guild and, if it was new, asks the scheduler
/// to sync its structure at once. Idempotent.
pub async fn follow(
    pool: &SqlitePool,
    sync: Option<&SyncHandle>,
    guild_id: &str,
    admin_id: &str,
    event_id: EventId,
    now: DateTime<Utc>,
) -> Result<FollowOutcome, FollowError> {
    let mut tx = store::begin_write(pool).await?;
    let event = store::events::get(&mut *tx, event_id)
        .await?
        .ok_or(FollowError::UnknownEvent(event_id))?;
    let newly_followed = store::guilds::follow_event(
        &mut *tx,
        &FollowedEvent {
            guild_id: guild_id.to_owned(),
            event_id,
            followed_by: admin_id.to_owned(),
            followed_at: now,
        },
    )
    .await?;
    if newly_followed {
        audit(&mut tx, "follow_event", guild_id, admin_id, event_id, now).await?;
    }
    tx.commit().await?;
    if newly_followed {
        match sync {
            Some(s) => s.sync_event_now(event_id),
            None => tracing::warn!(
                event = event_id.0,
                "followed an event without a running sync worker; it syncs on its next start"
            ),
        }
    }
    Ok(FollowOutcome {
        event,
        newly_followed,
    })
}

/// Unfollows an event in a guild. The scheduler stops syncing it once no
/// guild follows it. Picks, results and history are untouched.
pub async fn unfollow(
    pool: &SqlitePool,
    guild_id: &str,
    admin_id: &str,
    event_id: EventId,
    now: DateTime<Utc>,
) -> Result<UnfollowOutcome, FollowError> {
    let mut tx = store::begin_write(pool).await?;
    let event = store::events::get(&mut *tx, event_id)
        .await?
        .ok_or(FollowError::UnknownEvent(event_id))?;
    let was_followed = store::guilds::unfollow_event(&mut *tx, guild_id, event_id).await?;
    if was_followed {
        audit(&mut tx, "unfollow_event", guild_id, admin_id, event_id, now).await?;
    }
    tx.commit().await?;
    Ok(UnfollowOutcome {
        event,
        was_followed,
    })
}

async fn audit(
    conn: &mut sqlx::SqliteConnection,
    action: &str,
    guild_id: &str,
    admin_id: &str,
    event_id: EventId,
    now: DateTime<Utc>,
) -> Result<(), sqlx::Error> {
    store::audit::append(
        conn,
        &AuditRecord {
            actor: admin_id.to_owned(),
            action: action.to_owned(),
            entity_type: "event".to_owned(),
            entity_id: Some(event_id.0.to_string()),
            before: None,
            after: Some(json!({ "guild_id": guild_id })),
            at: now,
        },
    )
    .await?;
    Ok(())
}
