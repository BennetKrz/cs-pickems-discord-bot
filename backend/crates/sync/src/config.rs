//! Settings of the sync scheduler (T06). The app fills them from `[sync]`
//! in `config.toml` and `SYNC__*` environment variables; the defaults are
//! the ticket's cadences. See `docs/adr/0006-sync-scheduler-budget.md`.

use chrono::Duration;

/// Cadences and windows of the sync jobs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncConfig {
    /// Followed event structure (stages, bracket, match list).
    pub structure_every: Duration,
    /// Matches starting within this window are polled at `upcoming_every`.
    pub upcoming_window: Duration,
    pub upcoming_every: Duration,
    /// Matches starting within this window (or overdue by at most
    /// `overdue_window`) are polled at `imminent_every`.
    pub imminent_window: Duration,
    pub imminent_every: Duration,
    /// How long a match that should have started but is not live yet keeps
    /// the imminent cadence. After that only the structure job sees it.
    pub overdue_window: Duration,
    /// Live matches.
    pub live_every: Duration,
    /// Re-checks of a finished match, after its end.
    pub corrections_after: Vec<Duration>,
    /// Event search results are reused for this long.
    pub search_cache: Duration,
    /// Interval of the lock ticker.
    pub lock_every: Duration,
    /// How often the scheduler loop wakes up to look for due jobs.
    pub tick_every: Duration,
    /// Upper bound of the random delay of a job's first run after a start.
    pub startup_jitter: Duration,
    /// Earliest retry of a job whose last call failed.
    pub failure_backoff: Duration,
}

impl Default for SyncConfig {
    fn default() -> Self {
        Self {
            structure_every: Duration::hours(6),
            upcoming_window: Duration::hours(48),
            upcoming_every: Duration::minutes(30),
            imminent_window: Duration::hours(1),
            imminent_every: Duration::minutes(5),
            overdue_window: Duration::hours(6),
            live_every: Duration::minutes(2),
            corrections_after: vec![Duration::minutes(10), Duration::hours(2)],
            search_cache: Duration::hours(24),
            lock_every: Duration::seconds(30),
            tick_every: Duration::seconds(10),
            startup_jitter: Duration::seconds(60),
            failure_backoff: Duration::minutes(5),
        }
    }
}

impl SyncConfig {
    /// Checks that the cadences make sense; returns one message per problem.
    pub fn validate(&self) -> Vec<String> {
        let mut problems = Vec::new();
        let positive = [
            ("structure_every", self.structure_every),
            ("upcoming_window", self.upcoming_window),
            ("upcoming_every", self.upcoming_every),
            ("imminent_window", self.imminent_window),
            ("imminent_every", self.imminent_every),
            ("live_every", self.live_every),
            ("search_cache", self.search_cache),
            ("lock_every", self.lock_every),
            ("tick_every", self.tick_every),
        ];
        for (name, d) in positive {
            if d <= Duration::zero() {
                problems.push(format!("{name} must be greater than zero"));
            }
        }
        if self.overdue_window < Duration::zero() {
            problems.push("overdue_window must not be negative".to_owned());
        }
        if self.imminent_window >= self.upcoming_window {
            problems.push("imminent_window must be shorter than upcoming_window".to_owned());
        }
        if self
            .corrections_after
            .iter()
            .any(|d| *d <= Duration::zero())
        {
            problems.push("corrections_after must only contain positive durations".to_owned());
        }
        problems
    }

    /// The last correction re-check after a match's end.
    pub fn last_correction(&self) -> Duration {
        self.corrections_after
            .iter()
            .copied()
            .max()
            .unwrap_or_else(Duration::zero)
    }
}

/// The request budget of one source: what the scheduler may spend, usually
/// 50% of the plan.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BudgetLimits {
    pub per_hour: u32,
    pub per_minute: u32,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_valid_and_match_the_ticket() {
        let c = SyncConfig::default();
        assert!(c.validate().is_empty());
        assert_eq!(c.live_every, Duration::minutes(2));
        assert_eq!(c.last_correction(), Duration::hours(2));
    }

    #[test]
    fn nonsense_is_reported() {
        let c = SyncConfig {
            live_every: Duration::zero(),
            imminent_window: Duration::hours(50),
            corrections_after: vec![Duration::minutes(-1)],
            ..SyncConfig::default()
        };
        assert_eq!(c.validate().len(), 3);
    }
}
