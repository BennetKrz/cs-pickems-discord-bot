//! A hard cap on requests, as a safety net under the scheduler's budget
//! (T06): the adapter itself never sends more than the plan allows.

use std::collections::VecDeque;
use std::time::Duration;

use tokio::time::Instant;

use crate::RateLimits;

const MINUTE: Duration = Duration::from_secs(60);
const HOUR: Duration = Duration::from_secs(3600);

/// How long to hold back after the source reported `remaining: 0`. The
/// next request after that is a probe whose headers say whether the window
/// has reset.
pub const EXHAUSTED_PAUSE: Duration = MINUTE;

/// Sliding windows over the last minute and the last hour.
#[derive(Debug)]
pub struct Limiter {
    limits: RateLimits,
    minute: VecDeque<Instant>,
    hour: VecDeque<Instant>,
    paused_until: Option<Instant>,
}

impl Limiter {
    pub fn new(limits: RateLimits) -> Self {
        Self {
            limits,
            minute: VecDeque::new(),
            hour: VecDeque::new(),
            paused_until: None,
        }
    }

    /// Takes one request slot at `now`, or says how long to wait.
    pub fn try_acquire(&mut self, now: Instant) -> Result<(), Duration> {
        prune(&mut self.minute, now, MINUTE);
        prune(&mut self.hour, now, HOUR);

        let mut wait = Duration::ZERO;
        if let Some(until) = self.paused_until {
            if until > now {
                wait = wait.max(until - now);
            } else {
                self.paused_until = None;
            }
        }
        wait = wait.max(full_until(
            &self.minute,
            self.limits.per_minute,
            now,
            MINUTE,
        ));
        wait = wait.max(full_until(&self.hour, self.limits.per_hour, now, HOUR));
        if !wait.is_zero() {
            return Err(wait);
        }
        self.minute.push_back(now);
        self.hour.push_back(now);
        Ok(())
    }

    /// Records the source's own remaining count. At 0 the limiter holds
    /// back for [`EXHAUSTED_PAUSE`].
    pub fn note_remaining(&mut self, remaining: u32, now: Instant) {
        if remaining == 0 {
            self.paused_until = Some(now + EXHAUSTED_PAUSE);
        }
    }
}

fn prune(window: &mut VecDeque<Instant>, now: Instant, span: Duration) {
    while window
        .front()
        .is_some_and(|t| now.duration_since(*t) >= span)
    {
        window.pop_front();
    }
}

/// Zero if the window has room, else the time until its oldest entry
/// leaves.
fn full_until(window: &VecDeque<Instant>, cap: u32, now: Instant, span: Duration) -> Duration {
    if (window.len() as u64) < u64::from(cap) {
        return Duration::ZERO;
    }
    match window.front() {
        Some(oldest) => (*oldest + span).saturating_duration_since(now),
        // A cap of 0 allows nothing.
        None => span,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn limiter(per_minute: u32, per_hour: u32) -> Limiter {
        Limiter::new(RateLimits {
            per_minute,
            per_hour,
        })
    }

    #[test]
    fn per_minute_cap_slides() {
        let mut l = limiter(3, 100);
        let t0 = Instant::now();
        for i in 0..3 {
            l.try_acquire(t0 + Duration::from_secs(i)).unwrap();
        }
        let wait = l.try_acquire(t0 + Duration::from_secs(10)).unwrap_err();
        assert_eq!(wait, Duration::from_secs(50));
        // The first request leaves the window after 60 s.
        l.try_acquire(t0 + Duration::from_secs(60)).unwrap();
        assert!(l.try_acquire(t0 + Duration::from_secs(60)).is_err());
    }

    #[test]
    fn per_hour_cap_holds_across_minutes() {
        let mut l = limiter(60, 5);
        let t0 = Instant::now();
        for i in 0..5 {
            l.try_acquire(t0 + Duration::from_secs(i * 120)).unwrap();
        }
        let at = t0 + Duration::from_secs(1200);
        assert_eq!(l.try_acquire(at).unwrap_err(), Duration::from_secs(2400));
        l.try_acquire(t0 + HOUR).unwrap();
    }

    #[test]
    fn never_more_than_the_plan_in_any_minute() {
        let mut l = limiter(60, 1000);
        let t0 = Instant::now();
        let mut sent = Vec::new();
        // Try every 100 ms for 10 minutes.
        for i in 0..6000 {
            let now = t0 + Duration::from_millis(i * 100);
            if l.try_acquire(now).is_ok() {
                sent.push(now);
            }
        }
        for (i, start) in sent.iter().enumerate() {
            let in_window = sent[i..]
                .iter()
                .take_while(|t| t.duration_since(*start) < MINUTE)
                .count();
            assert!(in_window <= 60, "{in_window} requests within one minute");
        }
        assert_eq!(sent.len(), 600);
    }

    #[test]
    fn remaining_zero_pauses() {
        let mut l = limiter(60, 1000);
        let t0 = Instant::now();
        l.note_remaining(5, t0);
        l.try_acquire(t0).unwrap();
        l.note_remaining(0, t0);
        assert_eq!(l.try_acquire(t0).unwrap_err(), EXHAUSTED_PAUSE);
        l.try_acquire(t0 + EXHAUSTED_PAUSE).unwrap();
    }
}
