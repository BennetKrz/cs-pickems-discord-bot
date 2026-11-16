//! The in-process domain-event bus.
//!
//! A `tokio::sync::broadcast` channel: every subscriber gets every event
//! published after it subscribed. A subscriber that falls more than
//! [`CAPACITY`] events behind gets `RecvError::Lagged` and must re-read the
//! state it cares about from the database. The reconciler publishes only
//! after its transaction committed.

use domain::DomainEvent;
use tokio::sync::broadcast;

/// Events buffered per subscriber.
pub const CAPACITY: usize = 1024;

#[derive(Debug, Clone)]
pub struct EventBus {
    tx: broadcast::Sender<DomainEvent>,
}

impl Default for EventBus {
    fn default() -> Self {
        Self::new()
    }
}

impl EventBus {
    pub fn new() -> Self {
        let (tx, _) = broadcast::channel(CAPACITY);
        Self { tx }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<DomainEvent> {
        self.tx.subscribe()
    }

    /// Sends events to all current subscribers. Without subscribers the
    /// events are dropped.
    pub fn publish(&self, events: impl IntoIterator<Item = DomainEvent>) {
        for e in events {
            tracing::debug!(event = ?e, "domain event");
            // An error only means there is no subscriber right now.
            let _ = self.tx.send(e);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use domain::MatchId;

    #[test]
    fn subscribers_receive_published_events() {
        let bus = EventBus::new();
        bus.publish([DomainEvent::MatchStarted {
            match_id: MatchId(1),
        }]);
        let mut rx = bus.subscribe();
        bus.publish([DomainEvent::MatchFinished {
            match_id: MatchId(1),
        }]);
        assert_eq!(
            rx.try_recv().unwrap(),
            DomainEvent::MatchFinished {
                match_id: MatchId(1)
            }
        );
        assert!(rx.try_recv().is_err());
    }
}
