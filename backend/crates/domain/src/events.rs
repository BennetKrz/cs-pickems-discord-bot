//! Domain events: real changes to canonical match data.
//!
//! The reconciler (T03) emits them on an in-process broadcast channel after
//! the change is committed, and only when a value actually changed.
//! Admin actions (T05) emit void, unvoid and reopen the same way.
//! Consumers: lock (T10), settlement (T11), announcements (T08).

use chrono::{DateTime, Utc};

use crate::MatchId;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DomainEvent {
    /// A new canonical match. The only event for a match on its first sync,
    /// whatever its status; consumers read the current state from the DB.
    MatchCreated { match_id: MatchId },
    /// `scheduled_at` changed.
    MatchRescheduled {
        match_id: MatchId,
        from: Option<DateTime<Utc>>,
        to: Option<DateTime<Utc>>,
    },
    /// The status changed to `live`.
    MatchStarted { match_id: MatchId },
    /// A map's name, status, rounds or winner changed.
    MapUpdated { match_id: MatchId, number: u8 },
    /// The status changed to `finished`.
    MatchFinished { match_id: MatchId },
    /// The status changed to `canceled`.
    MatchCanceled { match_id: MatchId },
    /// The match was finished before and after, and its series or map
    /// result changed.
    ResultCorrected { match_id: MatchId },
    /// An admin voided the match (T05): it scores no points for anyone.
    MatchVoided { match_id: MatchId },
    /// An admin took a void back; the match scores normally again.
    MatchUnvoided { match_id: MatchId },
    /// An admin unlocked the match's picks before it went live, e.g. after
    /// a long postponement (T05).
    PicksReopened { match_id: MatchId },
}

impl DomainEvent {
    pub fn match_id(&self) -> MatchId {
        match self {
            DomainEvent::MatchCreated { match_id }
            | DomainEvent::MatchRescheduled { match_id, .. }
            | DomainEvent::MatchStarted { match_id }
            | DomainEvent::MapUpdated { match_id, .. }
            | DomainEvent::MatchFinished { match_id }
            | DomainEvent::MatchCanceled { match_id }
            | DomainEvent::ResultCorrected { match_id }
            | DomainEvent::MatchVoided { match_id }
            | DomainEvent::MatchUnvoided { match_id }
            | DomainEvent::PicksReopened { match_id } => *match_id,
        }
    }
}
