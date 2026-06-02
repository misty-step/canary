//! Server wall-clock formatting policy.
//!
//! Route handlers and lifecycle workers need two persisted clock shapes:
//! RFC3339 UTC strings for database/API fields and Unix milliseconds for the
//! health state machine. This module keeps those encodings out of the route
//! table and worker loops without introducing an injectable clock abstraction.

use time::{OffsetDateTime, format_description::well_known::Rfc3339};

const RFC3339_FALLBACK: &str = "1970-01-01T00:00:00Z";

/// Return the current UTC instant.
pub(crate) fn current_utc() -> OffsetDateTime {
    OffsetDateTime::now_utc()
}

/// Return the current UTC instant formatted for Canary's persisted timestamps.
pub(crate) fn current_rfc3339() -> String {
    format_rfc3339(current_utc())
}

/// Return the current UTC instant as Unix milliseconds.
pub(crate) fn current_unix_millis() -> i64 {
    unix_millis(current_utc())
}

/// Format an instant with the same fallback used by server lifecycle code.
pub(crate) fn format_rfc3339(instant: OffsetDateTime) -> String {
    instant
        .format(&Rfc3339)
        .unwrap_or_else(|_| RFC3339_FALLBACK.to_owned())
}

fn unix_millis(instant: OffsetDateTime) -> i64 {
    unix_millis_from_nanos(instant.unix_timestamp_nanos())
}

fn unix_millis_from_nanos(nanos: i128) -> i64 {
    i64::try_from(nanos / 1_000_000).unwrap_or(i64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_rfc3339_uses_utc_wire_shape() -> Result<(), Box<dyn std::error::Error>> {
        let instant = OffsetDateTime::parse("2026-05-29T12:00:00Z", &Rfc3339)?;

        assert_eq!(format_rfc3339(instant), "2026-05-29T12:00:00Z");

        Ok(())
    }

    #[test]
    fn unix_millis_saturates_when_nanos_exceed_i64_millis() {
        assert_eq!(unix_millis_from_nanos(i128::MAX), i64::MAX);
    }

    #[test]
    fn unix_millis_converts_representable_instants() -> Result<(), Box<dyn std::error::Error>> {
        let instant = OffsetDateTime::parse("2026-05-29T12:00:00Z", &Rfc3339)?;

        assert_eq!(unix_millis(instant), 1_780_056_000_000);

        Ok(())
    }
}
