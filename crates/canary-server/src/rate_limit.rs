//! Runtime fixed-window rate limiter.
//!
//! The HTTP crate owns policy and problem shape. This module owns process-local
//! buckets, matching Phoenix's ETS-backed limiter without introducing a
//! database table or generic middleware framework.

use std::{
    collections::BTreeMap,
    time::{Duration, Instant},
};

use canary_http::rate_limit::RateLimitKind;

/// Process-local fixed-window rate limiter.
#[derive(Debug, Default)]
pub(crate) struct RateLimiter {
    buckets: BTreeMap<RateLimitBucketKey, RateLimitBucket>,
}

impl RateLimiter {
    /// Check a request using the current monotonic clock.
    pub(crate) fn check(&mut self, kind: RateLimitKind, identity: &str) -> RateLimitDecision {
        self.check_at(kind, identity, Instant::now())
    }

    /// Check whether a request would be limited without consuming capacity.
    pub(crate) fn peek(&mut self, kind: RateLimitKind, identity: &str) -> RateLimitDecision {
        self.peek_at(kind, identity, Instant::now())
    }

    fn check_at(&mut self, kind: RateLimitKind, identity: &str, now: Instant) -> RateLimitDecision {
        let policy = kind.policy();
        self.buckets.retain(|key, bucket| {
            now.duration_since(bucket.window_start) < window_duration(key.kind)
        });
        let key = RateLimitBucketKey {
            kind,
            identity: identity.to_owned(),
        };

        match self.buckets.get_mut(&key) {
            Some(bucket) if now.duration_since(bucket.window_start) < window_duration(kind) => {
                if bucket.count >= policy.limit {
                    RateLimitDecision::Limited {
                        retry_after_seconds: retry_after_seconds(
                            window_duration(kind),
                            now.duration_since(bucket.window_start),
                        ),
                    }
                } else {
                    bucket.count += 1;
                    RateLimitDecision::Allowed
                }
            }
            _ => {
                self.buckets.insert(
                    key,
                    RateLimitBucket {
                        count: 1,
                        window_start: now,
                    },
                );
                RateLimitDecision::Allowed
            }
        }
    }

    fn peek_at(&mut self, kind: RateLimitKind, identity: &str, now: Instant) -> RateLimitDecision {
        self.buckets.retain(|key, bucket| {
            now.duration_since(bucket.window_start) < window_duration(key.kind)
        });
        let key = RateLimitBucketKey {
            kind,
            identity: identity.to_owned(),
        };

        match self.buckets.get(&key) {
            Some(bucket)
                if now.duration_since(bucket.window_start) < window_duration(kind)
                    && bucket.count >= kind.policy().limit =>
            {
                RateLimitDecision::Limited {
                    retry_after_seconds: retry_after_seconds(
                        window_duration(kind),
                        now.duration_since(bucket.window_start),
                    ),
                }
            }
            _ => RateLimitDecision::Allowed,
        }
    }
}

/// Result of one rate-limit check.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RateLimitDecision {
    /// Request may continue.
    Allowed,
    /// Request exceeded its bucket.
    Limited {
        /// Phoenix-compatible whole-second retry delay.
        retry_after_seconds: u64,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct RateLimitBucketKey {
    kind: RateLimitKind,
    identity: String,
}

#[derive(Debug, Clone)]
struct RateLimitBucket {
    count: u32,
    window_start: Instant,
}

fn window_duration(kind: RateLimitKind) -> Duration {
    Duration::from_millis(kind.policy().window_ms)
}

fn retry_after_seconds(window: Duration, elapsed: Duration) -> u64 {
    window.saturating_sub(elapsed).as_millis() as u64 / 1_000 + 1
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixed_window_allows_limit_then_reports_phoenix_retry_after() {
        let mut limiter = RateLimiter::default();
        let start = Instant::now();

        for _ in 0..30 {
            assert_eq!(
                limiter.check_at(RateLimitKind::Query, "KEY-read", start),
                RateLimitDecision::Allowed
            );
        }

        assert_eq!(
            limiter.check_at(
                RateLimitKind::Query,
                "KEY-read",
                start + Duration::from_millis(59_001)
            ),
            RateLimitDecision::Limited {
                retry_after_seconds: 1
            }
        );
    }

    #[test]
    fn fixed_window_resets_after_window_and_separates_kind_and_identity() {
        let mut limiter = RateLimiter::default();
        let start = Instant::now();

        for _ in 0..30 {
            let _ = limiter.check_at(RateLimitKind::Query, "KEY-read", start);
        }

        assert_eq!(
            limiter.check_at(RateLimitKind::Query, "KEY-other", start),
            RateLimitDecision::Allowed
        );
        assert_eq!(
            limiter.check_at(RateLimitKind::Ingest, "KEY-read", start),
            RateLimitDecision::Allowed
        );
        assert_eq!(
            limiter.check_at(
                RateLimitKind::Query,
                "KEY-read",
                start + Duration::from_millis(60_000)
            ),
            RateLimitDecision::Allowed
        );
    }

    #[test]
    fn expired_buckets_are_pruned_on_check() {
        let mut limiter = RateLimiter::default();
        let start = Instant::now();

        for index in 0..10 {
            assert_eq!(
                limiter.check_at(RateLimitKind::AuthFail, &format!("ip-{index}"), start),
                RateLimitDecision::Allowed
            );
        }
        assert_eq!(limiter.buckets.len(), 10);

        assert_eq!(
            limiter.check_at(
                RateLimitKind::AuthFail,
                "ip-current",
                start + Duration::from_millis(60_000)
            ),
            RateLimitDecision::Allowed
        );
        assert_eq!(limiter.buckets.len(), 1);
    }

    #[test]
    fn peek_reports_limited_without_consuming_capacity() {
        let mut limiter = RateLimiter::default();
        let start = Instant::now();

        for _ in 0..29 {
            assert_eq!(
                limiter.check_at(RateLimitKind::Query, "KEY-read", start),
                RateLimitDecision::Allowed
            );
        }

        assert_eq!(
            limiter.peek_at(RateLimitKind::Query, "KEY-read", start),
            RateLimitDecision::Allowed
        );
        assert_eq!(
            limiter.check_at(RateLimitKind::Query, "KEY-read", start),
            RateLimitDecision::Allowed
        );
        assert!(matches!(
            limiter.peek_at(RateLimitKind::Query, "KEY-read", start),
            RateLimitDecision::Limited { .. }
        ));
        assert!(matches!(
            limiter.check_at(RateLimitKind::Query, "KEY-read", start),
            RateLimitDecision::Limited { .. }
        ));
    }
}
