//! TLS-expiry scan planning.
//!
//! Runtime code owns loading targets, recording timeline events, and enqueueing
//! webhooks. This module owns the Phoenix-compatible decision for one persisted
//! TLS expiry timestamp.

use time::{OffsetDateTime, format_description::well_known::Rfc3339};

/// Phoenix warning threshold for persisted TLS certificate expiry.
pub const EXPIRY_WARNING_DAYS: i64 = 14;

/// Persisted TLS metadata for one active HTTPS target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TlsExpiryScanInput {
    /// Target id.
    pub target_id: String,
    /// Target display name.
    pub name: String,
    /// Service name resolved with Phoenix's target service fallback.
    pub service: String,
    /// Target URL.
    pub url: String,
    /// Latest persisted TLS certificate expiration timestamp.
    pub tls_expires_at: String,
}

/// Planned TLS-expiring event for one target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TlsExpiryEvent {
    /// Target id.
    pub target_id: String,
    /// Target display name.
    pub name: String,
    /// Service name resolved with Phoenix's target service fallback.
    pub service: String,
    /// Target URL.
    pub url: String,
    /// Persisted TLS certificate expiration timestamp.
    pub tls_expires_at: String,
    /// Whole days until certificate expiry.
    pub days_until_expiry: i64,
}

/// Decide whether a target's latest persisted TLS timestamp should emit the
/// Phoenix `health_check.tls_expiring` event.
pub fn plan_tls_expiry_event(
    input: TlsExpiryScanInput,
    now: OffsetDateTime,
) -> Option<TlsExpiryEvent> {
    let expiry = OffsetDateTime::parse(&input.tls_expires_at, &Rfc3339).ok()?;
    if expiry < now {
        return None;
    }
    let days_until_expiry = (expiry - now).whole_days();

    if (0..EXPIRY_WARNING_DAYS).contains(&days_until_expiry) {
        Some(TlsExpiryEvent {
            target_id: input.target_id,
            name: input.name,
            service: input.service,
            url: input.url,
            tls_expires_at: input.tls_expires_at,
            days_until_expiry,
        })
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(expiry: &str) -> TlsExpiryScanInput {
        TlsExpiryScanInput {
            target_id: "TGT-api".to_owned(),
            name: "api-web".to_owned(),
            service: "api".to_owned(),
            url: "https://api.example.test/healthz".to_owned(),
            tls_expires_at: expiry.to_owned(),
        }
    }

    #[test]
    fn plans_expiring_tls_inside_phoenix_warning_window() -> Result<(), String> {
        let now = OffsetDateTime::parse("2026-05-29T00:00:00Z", &Rfc3339)
            .map_err(|error| error.to_string())?;
        let event =
            plan_tls_expiry_event(input("2026-06-05T00:00:00Z"), now).ok_or("expected event")?;

        assert_eq!(event.days_until_expiry, 7);
        assert_eq!(event.service, "api");
        assert_eq!(event.tls_expires_at, "2026-06-05T00:00:00Z");
        Ok(())
    }

    #[test]
    fn skips_expired_far_future_and_malformed_tls_timestamps() -> Result<(), String> {
        let now = OffsetDateTime::parse("2026-05-29T00:00:00Z", &Rfc3339)
            .map_err(|error| error.to_string())?;

        assert_eq!(
            plan_tls_expiry_event(input("2026-05-28T23:59:59Z"), now),
            None
        );
        assert_eq!(
            plan_tls_expiry_event(input("2026-06-12T00:00:00Z"), now),
            None
        );
        assert_eq!(plan_tls_expiry_event(input("not-a-date"), now), None);
        Ok(())
    }
}
