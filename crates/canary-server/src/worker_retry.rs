//! One post-failure retry-delay oracle shared by every lifecycle worker.
//!
//! A worker's tick interval is its steady-state cadence, not its recovery
//! cadence. Reusing the tick after a failed pass ties recovery latency to the
//! slowest maintenance schedule in the process: that cost canary-993 the whole
//! public surface for a day, because `retention_prune` ticks every 24h.
//!
//! Keep this the only retry policy. Five loops call it, and per-loop copies
//! would drift.

use std::time::Duration as StdDuration;

/// Base delay after a failed lifecycle pass, before clamping to the tick.
///
/// Readiness stays impaired until a pass succeeds, so the first retry sets the
/// floor on recovery time. Doubling backs a persistently failing worker off
/// within seconds, so a short base costs a few extra attempts, not a tight
/// loop.
const RETRY_BASE: StdDuration = StdDuration::from_secs(1);

/// Longest delay this oracle imposes between retries.
const RETRY_CEILING: StdDuration = StdDuration::from_secs(15 * 60);

/// Return the delay before the worker's next attempt.
///
/// `consecutive_failures` counts failed passes with no success after them, so
/// zero means nothing is pending: either no pass has run yet, or the last pass
/// succeeded. Zero returns the steady-state `tick_interval`.
///
/// Each failure doubles the delay from [`RETRY_BASE`], clamped to
/// [`RETRY_CEILING`] and to `tick_interval`. A worker whose tick is shorter
/// than [`RETRY_BASE`] therefore keeps its own cadence and retries sooner.
///
/// `tick_interval` must be non-zero, which every worker enforces before
/// spawning. A zero tick spins the loop through the success path as well, so
/// no retry policy can repair it here.
pub(crate) fn retry_delay(tick_interval: StdDuration, consecutive_failures: u32) -> StdDuration {
    if consecutive_failures == 0 {
        return tick_interval;
    }
    let doublings = consecutive_failures.saturating_sub(1);
    RETRY_BASE
        .saturating_mul(2u32.saturating_pow(doublings))
        .min(RETRY_CEILING)
        .min(tick_interval)
}

#[cfg(test)]
mod tests {
    use super::*;

    const DAY: StdDuration = StdDuration::from_secs(24 * 60 * 60);

    #[test]
    fn repeated_failures_double_the_delay_up_to_the_ceiling() {
        assert_eq!(retry_delay(DAY, 1), StdDuration::from_secs(1));
        assert_eq!(retry_delay(DAY, 2), StdDuration::from_secs(2));
        assert_eq!(retry_delay(DAY, 3), StdDuration::from_secs(4));
        assert_eq!(retry_delay(DAY, 4), StdDuration::from_secs(8));
        assert_eq!(retry_delay(DAY, 10), StdDuration::from_secs(512));
        assert_eq!(retry_delay(DAY, 11), RETRY_CEILING);
        assert_eq!(retry_delay(DAY, 12), RETRY_CEILING);
    }

    /// The ceiling must hold for any failure count, including values whose
    /// doubling would overflow.
    #[test]
    fn the_ceiling_holds_for_extreme_failure_counts() {
        assert_eq!(retry_delay(DAY, 64), RETRY_CEILING);
        assert_eq!(retry_delay(DAY, u32::MAX), RETRY_CEILING);
    }

    /// A tick between the base and the ceiling caps its own backoff. This is
    /// the webhook drain in production at 5s: growth stops at its cadence, and
    /// a saturated backoff still lands there rather than at the ceiling.
    #[test]
    fn an_intermediate_tick_caps_the_backoff_at_its_own_cadence() {
        let tick = StdDuration::from_secs(5);

        assert_eq!(retry_delay(tick, 4), tick);
        assert_eq!(retry_delay(tick, 20), tick);
    }

    /// A worker that ticks at or below the retry base keeps its own cadence.
    /// Backoff may only shorten a wait, never lengthen one. The 100ms tick is
    /// the case that exercises the clamp; 1s sits exactly on [`RETRY_BASE`].
    #[test]
    fn a_fast_worker_never_slows_down_after_a_failure() {
        for tick in [StdDuration::from_millis(100), RETRY_BASE] {
            for failures in [1, 2, 3, 10, u32::MAX] {
                assert_eq!(retry_delay(tick, failures), tick);
            }
        }
    }

    /// A successful pass restores the steady-state cadence at every interval
    /// length, so recovery ends the backoff.
    #[test]
    fn success_restores_the_steady_state_interval() {
        for interval in [StdDuration::from_millis(10), StdDuration::from_secs(5), DAY] {
            assert_eq!(retry_delay(interval, 0), interval);
        }
    }
}
