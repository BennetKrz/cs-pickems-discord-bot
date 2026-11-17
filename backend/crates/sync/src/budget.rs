//! The request budget of one source: two token buckets (per hour and per
//! minute) and priority reserves. Pure: the caller passes `now`.
//!
//! A bucket holds at most its cap and refills continuously at cap per
//! period. A call is charged its cost **before** it is made. Lower
//! priorities must leave a reserve of the hourly bucket untouched, so when
//! the budget runs short they wait first. A single call that costs more
//! than a bucket's cap may go once that bucket is full, so nothing is
//! blocked forever. See `docs/adr/0006-sync-scheduler-budget.md`.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::config::BudgetLimits;

/// Job priorities, most urgent first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Priority {
    Live,
    Imminent,
    Corrections,
    Upcoming,
    Structure,
    Search,
}

impl Priority {
    /// Share of the hourly cap this priority must leave unspent.
    pub fn reserve(self) -> f64 {
        match self {
            Priority::Live => 0.0,
            Priority::Imminent => 0.05,
            Priority::Corrections => 0.10,
            Priority::Upcoming => 0.15,
            Priority::Structure => 0.20,
            Priority::Search => 0.30,
        }
    }
}

/// Why a call could not be afforded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shortfall {
    Hour,
    Minute,
}

/// The persisted state of the buckets (`sync_state.cursor` of job
/// `budget`).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct SavedBudget {
    pub hour_tokens: f64,
    pub minute_tokens: f64,
    pub at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Budget {
    limits: BudgetLimits,
    hour: f64,
    minute: f64,
    at: DateTime<Utc>,
}

const HOUR_SECS: f64 = 3600.0;
const MINUTE_SECS: f64 = 60.0;

impl Budget {
    /// Full buckets: a source that never spent anything.
    pub fn full(limits: BudgetLimits, now: DateTime<Utc>) -> Self {
        Self {
            limits,
            hour: f64::from(limits.per_hour),
            minute: f64::from(limits.per_minute),
            at: now,
        }
    }

    /// Restores saved buckets and refills them for the time since they were
    /// saved, so a restart never starts with more than was left.
    pub fn restore(limits: BudgetLimits, saved: SavedBudget, now: DateTime<Utc>) -> Self {
        let mut b = Self {
            limits,
            hour: saved.hour_tokens.min(f64::from(limits.per_hour)),
            minute: saved.minute_tokens.min(f64::from(limits.per_minute)),
            at: saved.at,
        };
        b.refill(now);
        b
    }

    pub fn limits(&self) -> BudgetLimits {
        self.limits
    }

    pub fn saved(&self) -> SavedBudget {
        SavedBudget {
            hour_tokens: self.hour,
            minute_tokens: self.minute,
            at: self.at,
        }
    }

    /// Tokens left in the hourly bucket at `now`, rounded down.
    pub fn hour_left(&mut self, now: DateTime<Utc>) -> u32 {
        self.refill(now);
        // Saturating float-to-int cast; the value is within 0..=cap.
        self.hour.max(0.0).floor() as u32
    }

    fn refill(&mut self, now: DateTime<Utc>) {
        if now <= self.at {
            // A clock that went backwards refills nothing.
            return;
        }
        let secs = (now - self.at).as_seconds_f64();
        let hour_cap = f64::from(self.limits.per_hour);
        let minute_cap = f64::from(self.limits.per_minute);
        self.hour = (self.hour + hour_cap * secs / HOUR_SECS).min(hour_cap);
        self.minute = (self.minute + minute_cap * secs / MINUTE_SECS).min(minute_cap);
        self.at = now;
    }

    /// Charges `cost` requests at `priority`, or says which bucket is short.
    pub fn try_spend(
        &mut self,
        cost: u32,
        priority: Priority,
        now: DateTime<Utc>,
    ) -> Result<(), Shortfall> {
        if cost == 0 {
            return Ok(());
        }
        self.refill(now);
        let cost = f64::from(cost);
        let hour_cap = f64::from(self.limits.per_hour);
        let minute_cap = f64::from(self.limits.per_minute);

        let reserve = (hour_cap * priority.reserve()).floor();
        let hour_ok = if cost > hour_cap {
            self.hour >= hour_cap
        } else {
            self.hour - cost >= reserve
        };
        if !hour_ok {
            return Err(Shortfall::Hour);
        }
        let minute_ok = if cost > minute_cap {
            self.minute >= minute_cap
        } else {
            self.minute >= cost
        };
        if !minute_ok {
            return Err(Shortfall::Minute);
        }
        self.hour -= cost;
        self.minute -= cost;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{Duration, TimeZone};

    fn t0() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 11, 20, 12, 0, 0).unwrap()
    }

    fn limits(per_hour: u32, per_minute: u32) -> BudgetLimits {
        BudgetLimits {
            per_hour,
            per_minute,
        }
    }

    #[test]
    fn minute_cap_refills_continuously() {
        let mut b = Budget::full(limits(500, 30), t0());
        for _ in 0..30 {
            b.try_spend(1, Priority::Live, t0()).unwrap();
        }
        assert_eq!(b.try_spend(1, Priority::Live, t0()), Err(Shortfall::Minute));
        // 30 per minute = one every 2 s.
        let later = t0() + Duration::seconds(2);
        b.try_spend(1, Priority::Live, later).unwrap();
        assert_eq!(
            b.try_spend(1, Priority::Live, later),
            Err(Shortfall::Minute)
        );
    }

    #[test]
    fn hour_cap_holds_across_minutes() {
        let mut b = Budget::full(limits(10, 30), t0());
        let mut sent = 0;
        // One attempt per minute for 30 minutes: the full 10, plus 10/h
        // refilled over 29 minutes (4.8), rounded down.
        for m in 0..30 {
            if b.try_spend(1, Priority::Live, t0() + Duration::minutes(m))
                .is_ok()
            {
                sent += 1;
            }
        }
        assert_eq!(sent, 14);
    }

    #[test]
    fn lower_priorities_leave_a_reserve() {
        let mut b = Budget::full(limits(100, 1000), t0());
        // Structure keeps 20 back.
        for _ in 0..80 {
            b.try_spend(1, Priority::Structure, t0()).unwrap();
        }
        assert_eq!(
            b.try_spend(1, Priority::Structure, t0()),
            Err(Shortfall::Hour)
        );
        assert_eq!(b.try_spend(1, Priority::Search, t0()), Err(Shortfall::Hour));
        // Corrections may still go down to 10, live to 0.
        for _ in 0..10 {
            b.try_spend(1, Priority::Corrections, t0()).unwrap();
        }
        assert!(b.try_spend(1, Priority::Corrections, t0()).is_err());
        for _ in 0..10 {
            b.try_spend(1, Priority::Live, t0()).unwrap();
        }
        assert!(b.try_spend(1, Priority::Live, t0()).is_err());
        assert_eq!(b.hour_left(t0()), 0);
    }

    #[test]
    fn zero_cost_is_always_free_and_oversized_calls_wait_for_a_full_bucket() {
        let mut b = Budget::full(limits(10, 2), t0());
        b.try_spend(0, Priority::Search, t0()).unwrap();
        b.try_spend(5, Priority::Live, t0()).unwrap();
        assert_eq!(b.hour_left(t0()), 5);
        // The minute bucket was full, so the 5-request call went through;
        // the next one waits until it is full again.
        assert_eq!(b.try_spend(3, Priority::Live, t0()), Err(Shortfall::Minute));
        b.try_spend(3, Priority::Live, t0() + Duration::minutes(3))
            .unwrap();
    }

    #[test]
    fn restore_refills_only_the_elapsed_time() {
        let mut b = Budget::full(limits(500, 1000), t0());
        for _ in 0..30 {
            b.try_spend(10, Priority::Live, t0()).unwrap();
        }
        let saved = b.saved();
        assert_eq!(saved.hour_tokens, 200.0);

        // Restarted 6 minutes later: 50 more hourly tokens, not a full bucket.
        let later = saved.at + Duration::minutes(6);
        let mut r = Budget::restore(limits(500, 1000), saved, later);
        assert_eq!(r.hour_left(later), 250);

        // A lower limit after a config change caps the restored tokens.
        let mut r = Budget::restore(limits(10, 30), saved, saved.at);
        assert_eq!(r.hour_left(saved.at), 10);
    }

    #[test]
    fn a_clock_going_backwards_refills_nothing() {
        let mut b = Budget::full(limits(10, 10), t0());
        b.try_spend(10, Priority::Live, t0()).unwrap();
        assert_eq!(b.hour_left(t0() - Duration::hours(2)), 0);
    }
}
