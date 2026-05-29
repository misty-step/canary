//! Retention prune planning.
//!
//! Runtime code owns scheduling and logging. The store owns the SQL deletes.
//! This module owns the policy-to-cutoff decision so agents do not duplicate
//! date arithmetic across server workers or tests.

use time::{Duration, OffsetDateTime, format_description::well_known::Rfc3339};

/// Phoenix-compatible retention policy in days.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RetentionPolicy {
    /// Days to retain errors and service events.
    pub error_retention_days: i64,
    /// Days to retain target checks.
    pub check_retention_days: i64,
}

impl Default for RetentionPolicy {
    fn default() -> Self {
        Self {
            error_retention_days: 30,
            check_retention_days: 7,
        }
    }
}

/// Store command inputs for one retention prune pass.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RetentionPrunePlan {
    /// Delete errors and service events older than this timestamp.
    pub error_cutoff: String,
    /// Delete target checks older than this timestamp.
    pub check_cutoff: String,
}

/// Build the retention prune cutoffs from one observed clock value.
pub fn plan_retention_prune(
    policy: RetentionPolicy,
    now: OffsetDateTime,
) -> Result<RetentionPrunePlan, String> {
    if policy.error_retention_days < 0 {
        return Err("error retention days must be non-negative".to_owned());
    }
    if policy.check_retention_days < 0 {
        return Err("check retention days must be non-negative".to_owned());
    }

    Ok(RetentionPrunePlan {
        error_cutoff: format_rfc3339(now - Duration::days(policy.error_retention_days))?,
        check_cutoff: format_rfc3339(now - Duration::days(policy.check_retention_days))?,
    })
}

fn format_rfc3339(value: OffsetDateTime) -> Result<String, String> {
    value
        .format(&Rfc3339)
        .map_err(|error| format!("failed to format retention cutoff: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plan_retention_prune_uses_one_clock_for_both_cutoffs() -> Result<(), String> {
        let now = OffsetDateTime::parse("2026-05-29T12:00:00Z", &Rfc3339)
            .map_err(|error| error.to_string())?;
        let plan = plan_retention_prune(
            RetentionPolicy {
                error_retention_days: 30,
                check_retention_days: 7,
            },
            now,
        )?;

        assert_eq!(plan.error_cutoff, "2026-04-29T12:00:00Z");
        assert_eq!(plan.check_cutoff, "2026-05-22T12:00:00Z");
        Ok(())
    }

    #[test]
    fn plan_retention_prune_rejects_negative_days() -> Result<(), String> {
        let now = OffsetDateTime::parse("2026-05-29T12:00:00Z", &Rfc3339)
            .map_err(|error| error.to_string())?;

        assert_eq!(
            plan_retention_prune(
                RetentionPolicy {
                    error_retention_days: -1,
                    check_retention_days: 7,
                },
                now,
            ),
            Err("error retention days must be non-negative".to_owned())
        );
        assert_eq!(
            plan_retention_prune(
                RetentionPolicy {
                    error_retention_days: 30,
                    check_retention_days: -1,
                },
                now,
            ),
            Err("check retention days must be non-negative".to_owned())
        );
        Ok(())
    }
}
