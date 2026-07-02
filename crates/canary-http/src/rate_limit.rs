//! HTTP rate-limit contracts.
//!
//! The Axum runtime maintains in-memory fixed-window buckets. This module
//! keeps the public policy and 429 problem shape in one place.

use serde_json::json;

use crate::problem_details::{ProblemCode, ProblemDetails};

/// Rate-limit buckets enforced by the router and auth pipeline.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum RateLimitKind {
    /// Error and monitor check-in ingest routes.
    Ingest,
    /// Read/query routes plus admin routes in the query pipeline.
    Query,
    /// Invalid-key attempts grouped by client identity.
    AuthFail,
}

/// Fixed-window policy for one rate-limit bucket.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RateLimitPolicy {
    /// Maximum number of accepted requests inside the window.
    pub limit: u32,
    /// Window duration in milliseconds.
    pub window_ms: u64,
}

impl RateLimitKind {
    /// Return the limit and window for this bucket.
    pub const fn policy(self) -> RateLimitPolicy {
        match self {
            Self::Ingest => RateLimitPolicy {
                limit: 100,
                window_ms: 60_000,
            },
            Self::Query => RateLimitPolicy {
                limit: 30,
                window_ms: 60_000,
            },
            Self::AuthFail => RateLimitPolicy {
                limit: 10,
                window_ms: 60_000,
            },
        }
    }
}

/// Build the 429 body for an exhausted bucket.
pub fn rate_limited_problem(retry_after_seconds: u64) -> ProblemDetails {
    ProblemDetails::new(
        429,
        ProblemCode::RateLimited,
        format!("Rate limit exceeded. Try again in {retry_after_seconds} seconds."),
        None,
    )
    .with_extra("retry_after", json!(retry_after_seconds))
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, to_value};

    use super::*;

    #[test]
    fn policies_match_rate_limiter_constants() {
        assert_eq!(
            RateLimitKind::Ingest.policy(),
            RateLimitPolicy {
                limit: 100,
                window_ms: 60_000
            }
        );
        assert_eq!(
            RateLimitKind::Query.policy(),
            RateLimitPolicy {
                limit: 30,
                window_ms: 60_000
            }
        );
        assert_eq!(
            RateLimitKind::AuthFail.policy(),
            RateLimitPolicy {
                limit: 10,
                window_ms: 60_000
            }
        );
    }

    #[test]
    fn rate_limited_problem_matches_wire_shape() {
        let encoded = to_value(rate_limited_problem(42)).unwrap_or(Value::Null);

        assert_eq!(encoded["type"], "https://canary.dev/problems/rate-limited");
        assert_eq!(encoded["title"], "Rate Limit Exceeded");
        assert_eq!(encoded["status"], 429);
        assert_eq!(encoded["code"], "rate_limited");
        assert_eq!(
            encoded["detail"],
            "Rate limit exceeded. Try again in 42 seconds."
        );
        assert_eq!(encoded["retry_after"], 42);
        assert!(encoded["request_id"].is_null());
    }
}
