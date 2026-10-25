//! Bounds on what one caller can ask of the server: a token bucket per actor, and a cap on how many
//! event waits are open at once. Neither holds the store lock, and neither allocates per request.

use std::collections::HashMap;

use super::Error;

/// Requests per second an actor may sustain, and the burst it may spend at once.
pub(crate) struct Rates {
    pub read_per_second: f64,
    pub read_burst: f64,
    pub write_per_second: f64,
    pub write_burst: f64,
    /// Failed authentications, shared by every caller that has no valid credential.
    pub anonymous_per_second: f64,
    pub anonymous_burst: f64,
}

pub(crate) const RATES: Rates = Rates {
    read_per_second: 50.0,
    read_burst: 200.0,
    write_per_second: 20.0,
    write_burst: 60.0,
    anonymous_per_second: 5.0,
    anonymous_burst: 20.0,
};

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub(crate) enum Class {
    Read,
    Write,
}

struct Bucket {
    tokens: f64,
    last: f64,
}

#[derive(Default)]
pub(crate) struct RateLimiter {
    buckets: HashMap<(String, Class), Bucket>,
}

/// Idle buckets are dropped past this many, so a stream of distinct names cannot grow the table.
const MAX_BUCKETS: usize = 4096;

impl RateLimiter {
    /// Takes one token for `who` at time `now` (seconds on any monotonic clock).
    pub(crate) fn take(&mut self, who: &str, class: Class, now: f64) -> Result<(), Error> {
        let (rate, burst) = match (who, class) {
            ("", _) => (RATES.anonymous_per_second, RATES.anonymous_burst),
            (_, Class::Read) => (RATES.read_per_second, RATES.read_burst),
            (_, Class::Write) => (RATES.write_per_second, RATES.write_burst),
        };
        if self.buckets.len() >= MAX_BUCKETS {
            self.buckets.retain(|_, bucket| now - bucket.last < 60.0);
        }
        let bucket = self
            .buckets
            .entry((who.to_owned(), class))
            .or_insert(Bucket {
                tokens: burst,
                last: now,
            });
        bucket.tokens = (bucket.tokens + (now - bucket.last).max(0.0) * rate).min(burst);
        bucket.last = now;
        if bucket.tokens >= 1.0 {
            bucket.tokens -= 1.0;
            return Ok(());
        }
        let wait_ms = (((1.0 - bucket.tokens) / rate) * 1000.0).ceil() as u64;
        Err(Error::new("rate_limited", "too many requests").retry_after(wait_ms.max(1)))
    }
}

pub(crate) const MAX_WAITERS: usize = 64;
pub(crate) const MAX_WAITERS_PER_ACTOR: usize = 4;
/// A wait never holds a connection longer than this, whatever the caller asks for.
pub(crate) const MAX_WAIT_MS: u64 = 25_000;

#[derive(Default)]
pub(crate) struct Waiters {
    total: usize,
    per_actor: HashMap<String, usize>,
}

impl Waiters {
    pub(crate) fn enter(&mut self, actor: &str) -> Result<(), Error> {
        let mine = self.per_actor.get(actor).copied().unwrap_or(0);
        if self.total >= MAX_WAITERS || mine >= MAX_WAITERS_PER_ACTOR {
            return Err(Error::new(
                "too_many_subscriptions",
                "too many open event waits; poll with timeout_ms 0 or retry shortly",
            )
            .retry_after(1000));
        }
        self.total += 1;
        *self.per_actor.entry(actor.to_owned()).or_insert(0) += 1;
        Ok(())
    }

    pub(crate) fn leave(&mut self, actor: &str) {
        self.total = self.total.saturating_sub(1);
        if let Some(count) = self.per_actor.get_mut(actor) {
            *count = count.saturating_sub(1);
            if *count == 0 {
                self.per_actor.remove(actor);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_burst_is_allowed_then_refused_with_a_wait_that_works() {
        let mut limiter = RateLimiter::default();
        for _ in 0..60 {
            limiter.take("lead", Class::Write, 0.0).unwrap();
        }
        let error = limiter.take("lead", Class::Write, 0.0).unwrap_err();
        assert_eq!(error.code, "rate_limited");
        let wait = error.retry_after_ms.unwrap();
        assert!((40..=60).contains(&wait), "{wait}");
        // Waiting what was said is enough for exactly one more request.
        limiter
            .take("lead", Class::Write, f64::from(wait as u32) / 1000.0)
            .unwrap();
        assert!(limiter
            .take("lead", Class::Write, f64::from(wait as u32) / 1000.0)
            .is_err());
    }

    #[test]
    fn actors_and_classes_do_not_share_buckets() {
        let mut limiter = RateLimiter::default();
        for _ in 0..60 {
            limiter.take("a", Class::Write, 0.0).unwrap();
        }
        assert!(limiter.take("a", Class::Write, 0.0).is_err());
        limiter.take("b", Class::Write, 0.0).unwrap();
        limiter.take("a", Class::Read, 0.0).unwrap();
    }

    #[test]
    fn tokens_refill_but_never_past_the_burst() {
        let mut limiter = RateLimiter::default();
        for _ in 0..60 {
            limiter.take("a", Class::Write, 0.0).unwrap();
        }
        // A long idle period restores the burst, not more.
        for _ in 0..60 {
            limiter.take("a", Class::Write, 1000.0).unwrap();
        }
        assert!(limiter.take("a", Class::Write, 1000.0).is_err());
    }

    #[test]
    fn anonymous_failures_share_one_small_bucket() {
        let mut limiter = RateLimiter::default();
        for _ in 0..20 {
            limiter.take("", Class::Read, 0.0).unwrap();
        }
        assert_eq!(
            limiter.take("", Class::Read, 0.0).unwrap_err().code,
            "rate_limited"
        );
        limiter.take("real-actor", Class::Read, 0.0).unwrap();
    }

    #[test]
    fn distinct_names_cannot_grow_the_table_without_bound() {
        let mut limiter = RateLimiter::default();
        for i in 0..(MAX_BUCKETS * 2) {
            limiter
                .take(&format!("actor-{i}"), Class::Read, i as f64)
                .unwrap();
        }
        assert!(limiter.buckets.len() <= MAX_BUCKETS + 1);
    }

    #[test]
    fn open_waits_are_bounded_overall_and_per_actor() {
        let mut waiters = Waiters::default();
        for _ in 0..MAX_WAITERS_PER_ACTOR {
            waiters.enter("a").unwrap();
        }
        let error = waiters.enter("a").unwrap_err();
        assert_eq!(error.code, "too_many_subscriptions");
        assert!(error.retry_after_ms.is_some());
        waiters.leave("a");
        waiters.enter("a").unwrap();
        for i in 0..(MAX_WAITERS - MAX_WAITERS_PER_ACTOR) {
            waiters.enter(&format!("other-{i}")).unwrap();
        }
        assert!(waiters.enter("fresh").is_err(), "the global cap holds too");
        for _ in 0..MAX_WAITERS_PER_ACTOR {
            waiters.leave("a");
        }
        waiters.enter("fresh").unwrap();
    }
}
