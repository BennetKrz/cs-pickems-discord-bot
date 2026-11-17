//! The source of "now".
//!
//! Code that needs the current time takes a [`Clock`] (or a `now` argument)
//! instead of calling `Utc::now()`, so the fixture source and the dev clock
//! (T05) and the scheduler tests (T06) can run on a [`FakeClock`].

use std::sync::Mutex;

use chrono::{DateTime, Duration, Utc};

/// The current time in UTC.
pub trait Clock: Send + Sync {
    fn now(&self) -> DateTime<Utc>;
}

/// The real wall clock.
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> DateTime<Utc> {
        Utc::now()
    }
}

/// A frozen clock that only moves when told to. For tests and the dev
/// fixture scenario.
#[derive(Debug)]
pub struct FakeClock {
    now: Mutex<DateTime<Utc>>,
}

impl FakeClock {
    pub fn new(at: DateTime<Utc>) -> Self {
        Self {
            now: Mutex::new(at),
        }
    }

    /// Jumps to `at`, also backwards.
    pub fn set(&self, at: DateTime<Utc>) {
        *self.lock() = at;
    }

    /// Moves forward by `by` and returns the new time.
    pub fn advance(&self, by: Duration) -> DateTime<Utc> {
        let mut now = self.lock();
        *now += by;
        *now
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, DateTime<Utc>> {
        // A panic while holding the lock cannot leave a DateTime half
        // written, so a poisoned lock is still usable.
        self.now.lock().unwrap_or_else(|e| e.into_inner())
    }
}

impl Clock for FakeClock {
    fn now(&self) -> DateTime<Utc> {
        *self.lock()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn fake_clock_stands_still_until_moved() {
        let t0 = Utc.with_ymd_and_hms(2026, 11, 20, 18, 0, 0).unwrap();
        let c = FakeClock::new(t0);
        assert_eq!(c.now(), t0);
        assert_eq!(c.now(), t0);
        assert_eq!(c.advance(Duration::minutes(90)), t0 + Duration::minutes(90));
        assert_eq!(c.now(), t0 + Duration::minutes(90));
        c.set(t0);
        assert_eq!(c.now(), t0);

        let dyn_clock: &dyn Clock = &c;
        assert_eq!(dyn_clock.now(), t0);
    }

    #[test]
    fn system_clock_is_close_to_now() {
        let d = SystemClock.now() - Utc::now();
        assert!(d.num_seconds().abs() < 5);
    }
}
