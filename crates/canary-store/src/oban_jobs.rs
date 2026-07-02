//! Webhook delivery job persistence.
//!
//! The `oban_jobs` SQLite table and the `Canary.Workers.WebhookDelivery` worker
//! string are inherited from the deleted Elixir/Oban implementation. They are
//! kept as-is because:
//!
//! 1. **Production databases already have rows in `oban_jobs`.** Renaming the
//!    table requires a data migration that touches the single-writer store
//!    during a deploy window. The risk/reward is not yet justified — the table
//!    works correctly and the name does not affect runtime behavior.
//!
//! 2. **The `Canary.Workers.WebhookDelivery` worker string is persisted in
//!    existing `oban_jobs.worker` rows.** Changing it requires an `UPDATE`
//!    migration that could leave stale rows unclaimable if it runs
//!    partially.
//!
//! 3. **The legacy fixture compat tests** (`tests/legacy_fixture_compat.rs`)
//!    verify that the Rust migration path correctly handles databases created
//!    by the Elixir app. Renaming the table would break that compatibility path
//!    until the fixtures are regenerated.
//!
//! When the production database is next restamped (all Elixir-era rows are
//! drained), this table should be renamed to `webhook_delivery_jobs` and the
//! worker string updated to a Rust-idiomatic name. That is a coordinated
//! migration tracked by 066 child 2.

use rusqlite::{Connection, OptionalExtension, params};
use serde_json::Value;

use crate::Result;

const WEBHOOK_QUEUE: &str = "webhooks";
const WEBHOOK_WORKER: &str = "Canary.Workers.WebhookDelivery";
const WEBHOOK_PRIORITY: i64 = 1;

/// Oban state values relevant to webhook delivery jobs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WebhookDeliveryJobState {
    /// Job is ready to run.
    Available,
    /// Job is delayed until `scheduled_at`.
    Scheduled,
    /// Job is currently claimed by the Rust drain.
    Executing,
    /// Job completed successfully or was intentionally skipped.
    Completed,
    /// Job exhausted retries or has invalid arguments.
    Discarded,
}

impl WebhookDeliveryJobState {
    /// Persisted Oban state string.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Available => "available",
            Self::Scheduled => "scheduled",
            Self::Executing => "executing",
            Self::Completed => "completed",
            Self::Discarded => "discarded",
        }
    }

    fn from_str(value: &str) -> Self {
        match value {
            "scheduled" => Self::Scheduled,
            "executing" => Self::Executing,
            "completed" => Self::Completed,
            "discarded" => Self::Discarded,
            _ => Self::Available,
        }
    }
}

/// Fields required to insert one webhook delivery job.
#[derive(Debug, Clone, PartialEq)]
pub struct WebhookDeliveryJobInsert {
    /// Job args.
    pub args: Value,
    /// Initial scheduled timestamp.
    pub scheduled_at: String,
    /// Insert timestamp.
    pub now: String,
    /// Maximum attempts before final discard.
    pub max_attempts: u32,
}

/// Claimed or inspected webhook delivery job row.
#[derive(Debug, Clone, PartialEq)]
pub struct WebhookDeliveryJobRow {
    /// Oban job id.
    pub id: i64,
    /// Current state.
    pub state: WebhookDeliveryJobState,
    /// Args.
    pub args: Value,
    /// Current one-based attempt after claiming.
    pub attempt: u32,
    /// Maximum attempts before final discard.
    pub max_attempts: u32,
    /// Scheduled timestamp.
    pub scheduled_at: String,
    /// Timestamp for the currently claimed execution lease, when claimed.
    pub attempted_at: Option<String>,
}

/// Point-in-time summary of due webhook delivery backlog.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WebhookDeliveryJobDueSummary {
    /// Rows eligible for claiming at the observed timestamp.
    pub due_count: u32,
    /// Rows currently leased by a webhook delivery executor.
    pub in_flight_count: u32,
    /// Oldest eligible scheduled timestamp.
    pub oldest_scheduled_at: Option<String>,
}

/// Summary of stale executing webhook jobs recovered back into scheduler control.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WebhookDeliveryJobRecoveryReport {
    /// Executing rows selected as stale.
    pub recovered: u32,
    /// Rows leased back to the scheduled state.
    pub retried: u32,
    /// Rows discarded because they had exhausted attempts.
    pub discarded: u32,
}

/// Scheduler-side completion transition for one claimed webhook delivery job.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WebhookDeliveryJobCompletion {
    /// Reschedule the same job row for a later attempt.
    Retry {
        /// Next scheduled timestamp.
        scheduled_at: String,
    },
    /// Mark the job completed successfully or intentionally skipped.
    Complete {
        /// Completion timestamp.
        now: String,
    },
    /// Mark the job permanently discarded.
    Discard {
        /// Discard timestamp.
        now: String,
    },
}

pub(crate) fn insert_webhook_delivery_job(
    connection: &mut Connection,
    job: WebhookDeliveryJobInsert,
) -> Result<i64> {
    let args = serde_json::to_string(&job.args).map_err(|_| rusqlite::Error::InvalidQuery)?;
    let state = if job.scheduled_at <= job.now {
        WebhookDeliveryJobState::Available
    } else {
        WebhookDeliveryJobState::Scheduled
    };

    connection.execute(
        "INSERT INTO oban_jobs (
            state, queue, worker, args, max_attempts, priority, inserted_at, scheduled_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![
            state.as_str(),
            WEBHOOK_QUEUE,
            WEBHOOK_WORKER,
            args,
            i64::from(job.max_attempts),
            WEBHOOK_PRIORITY,
            job.now,
            job.scheduled_at,
        ],
    )?;
    Ok(connection.last_insert_rowid())
}

pub(crate) fn claim_due_webhook_delivery_jobs(
    connection: &mut Connection,
    now: &str,
    limit: u32,
) -> Result<Vec<WebhookDeliveryJobRow>> {
    if limit == 0 {
        return Ok(Vec::new());
    }

    let transaction = connection.transaction()?;
    let selected = {
        let mut statement = transaction.prepare(
            "SELECT id, scheduled_at
             FROM oban_jobs
             WHERE worker = ?1
               AND queue = ?2
               AND state IN ('available', 'scheduled')
               AND scheduled_at <= ?3
             ORDER BY priority ASC, scheduled_at ASC, id ASC
             LIMIT ?4",
        )?;
        let rows = statement
            .query_map(params![WEBHOOK_WORKER, WEBHOOK_QUEUE, now, limit], |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
            })?;
        rows.collect::<std::result::Result<Vec<_>, _>>()?
    };

    let mut ids = Vec::with_capacity(selected.len());
    for (id, scheduled_at) in selected {
        let updated = transaction.execute(
            "UPDATE oban_jobs
             SET state = 'executing',
                 attempt = attempt + 1,
                 attempted_at = ?2,
                 attempted_by = '[\"canary-rust\"]'
             WHERE id = ?1
               AND worker = ?3
               AND queue = ?4
               AND state IN ('available', 'scheduled')
               AND scheduled_at = ?5",
            params![id, now, WEBHOOK_WORKER, WEBHOOK_QUEUE, scheduled_at],
        )?;
        if updated > 0 {
            ids.push(id);
        }
    }

    let mut jobs = Vec::with_capacity(ids.len());
    for id in ids {
        if let Some(job) = webhook_delivery_job_in_transaction(&transaction, id)? {
            jobs.push(job);
        }
    }
    transaction.commit()?;
    Ok(jobs)
}

pub(crate) fn complete_webhook_delivery_job(
    connection: &mut Connection,
    job: &WebhookDeliveryJobRow,
    completion: WebhookDeliveryJobCompletion,
) -> Result<bool> {
    let Some(attempted_at) = job.attempted_at.as_deref() else {
        return Ok(false);
    };
    match completion {
        WebhookDeliveryJobCompletion::Retry { scheduled_at } => {
            let updated = connection.execute(
                "UPDATE oban_jobs
                 SET state = 'scheduled', scheduled_at = ?2
                 WHERE id = ?1
                   AND worker = ?3
                   AND queue = ?4
                   AND state = 'executing'
                   AND attempt = ?5
                   AND attempted_at = ?6",
                params![
                    job.id,
                    scheduled_at,
                    WEBHOOK_WORKER,
                    WEBHOOK_QUEUE,
                    job.attempt,
                    attempted_at
                ],
            )?;
            Ok(updated > 0)
        }
        WebhookDeliveryJobCompletion::Complete { now } => {
            let updated = connection.execute(
                "UPDATE oban_jobs
                 SET state = 'completed', completed_at = ?2
                 WHERE id = ?1
                   AND worker = ?3
                   AND queue = ?4
                   AND state = 'executing'
                   AND attempt = ?5
                   AND attempted_at = ?6",
                params![
                    job.id,
                    now,
                    WEBHOOK_WORKER,
                    WEBHOOK_QUEUE,
                    job.attempt,
                    attempted_at
                ],
            )?;
            Ok(updated > 0)
        }
        WebhookDeliveryJobCompletion::Discard { now } => {
            let updated = connection.execute(
                "UPDATE oban_jobs
                 SET state = 'discarded', discarded_at = ?2
                 WHERE id = ?1
                   AND worker = ?3
                   AND queue = ?4
                   AND state = 'executing'
                   AND attempt = ?5
                   AND attempted_at = ?6",
                params![
                    job.id,
                    now,
                    WEBHOOK_WORKER,
                    WEBHOOK_QUEUE,
                    job.attempt,
                    attempted_at
                ],
            )?;
            Ok(updated > 0)
        }
    }
}

pub(crate) fn recover_stale_webhook_delivery_jobs(
    connection: &mut Connection,
    now: &str,
    stale_before: &str,
    limit: u32,
) -> Result<WebhookDeliveryJobRecoveryReport> {
    if limit == 0 {
        return Ok(WebhookDeliveryJobRecoveryReport::default());
    }

    let transaction = connection.transaction()?;
    let stale = {
        let mut statement = transaction.prepare(
            "SELECT id, attempt, max_attempts, errors, attempted_at
             FROM oban_jobs
             WHERE worker = ?1
               AND queue = ?2
               AND state = 'executing'
               AND attempted_at IS NOT NULL
               AND attempted_at <= ?3
             ORDER BY attempted_at ASC, id ASC
             LIMIT ?4",
        )?;
        let rows = statement.query_map(
            params![WEBHOOK_WORKER, WEBHOOK_QUEUE, stale_before, limit],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, u32>(1)?,
                    row.get::<_, u32>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                ))
            },
        )?;
        rows.collect::<std::result::Result<Vec<_>, _>>()?
    };

    let mut report = WebhookDeliveryJobRecoveryReport::default();
    for (id, attempt, max_attempts, errors, attempted_at) in stale {
        let errors = append_recovery_error(errors, now, stale_before)?;
        if attempt >= max_attempts {
            let updated = transaction.execute(
                "UPDATE oban_jobs
                 SET state = 'discarded',
                     discarded_at = ?2,
                     errors = ?3
                 WHERE id = ?1
                   AND worker = ?4
                   AND queue = ?5
                   AND state = 'executing'
                   AND attempted_at = ?6",
                params![id, now, errors, WEBHOOK_WORKER, WEBHOOK_QUEUE, attempted_at],
            )?;
            if updated > 0 {
                report.discarded += 1;
                report.recovered += 1;
            }
        } else {
            let updated = transaction.execute(
                "UPDATE oban_jobs
                 SET state = 'scheduled',
                     scheduled_at = ?2,
                     errors = ?3
                 WHERE id = ?1
                   AND worker = ?4
                   AND queue = ?5
                   AND state = 'executing'
                   AND attempted_at = ?6",
                params![id, now, errors, WEBHOOK_WORKER, WEBHOOK_QUEUE, attempted_at],
            )?;
            if updated > 0 {
                report.retried += 1;
                report.recovered += 1;
            }
        }
    }
    transaction.commit()?;
    Ok(report)
}

pub(crate) fn webhook_delivery_due_summary(
    connection: &Connection,
    now: &str,
) -> Result<WebhookDeliveryJobDueSummary> {
    let mut statement = connection.prepare(
        "SELECT
           COUNT(CASE WHEN state IN ('available', 'scheduled') AND scheduled_at <= ?3 THEN 1 END),
           MIN(CASE WHEN state IN ('available', 'scheduled') AND scheduled_at <= ?3 THEN scheduled_at END),
           COUNT(CASE WHEN state = 'executing' THEN 1 END)
         FROM oban_jobs
         WHERE worker = ?1
           AND queue = ?2",
    )?;
    statement
        .query_row(params![WEBHOOK_WORKER, WEBHOOK_QUEUE, now], |row| {
            Ok(WebhookDeliveryJobDueSummary {
                due_count: row.get::<_, u32>(0)?,
                oldest_scheduled_at: row.get::<_, Option<String>>(1)?,
                in_flight_count: row.get::<_, u32>(2)?,
            })
        })
        .map_err(Into::into)
}

pub(crate) fn webhook_delivery_job(
    connection: &Connection,
    job_id: i64,
) -> Result<Option<WebhookDeliveryJobRow>> {
    let mut statement = connection.prepare(
        "SELECT id, state, args, attempt, max_attempts, scheduled_at, attempted_at
         FROM oban_jobs
         WHERE id = ?1 AND worker = ?2 AND queue = ?3",
    )?;
    statement
        .query_row(params![job_id, WEBHOOK_WORKER, WEBHOOK_QUEUE], row)
        .optional()
        .map_err(Into::into)
}

fn webhook_delivery_job_in_transaction(
    connection: &Connection,
    job_id: i64,
) -> Result<Option<WebhookDeliveryJobRow>> {
    webhook_delivery_job(connection, job_id)
}

fn append_recovery_error(errors: String, now: &str, stale_before: &str) -> Result<String> {
    let mut errors = serde_json::from_str::<Vec<Value>>(&errors).unwrap_or_default();
    errors.push(serde_json::json!({
        "at": now,
        "reason": "stale_executing_recovered",
        "stale_before": stale_before
    }));
    serde_json::to_string(&errors).map_err(|_| rusqlite::Error::InvalidQuery.into())
}

fn row(row: &rusqlite::Row<'_>) -> rusqlite::Result<WebhookDeliveryJobRow> {
    let state: String = row.get(1)?;
    let args: String = row.get(2)?;
    Ok(WebhookDeliveryJobRow {
        id: row.get(0)?,
        state: WebhookDeliveryJobState::from_str(&state),
        args: serde_json::from_str(&args).map_err(|_| rusqlite::Error::InvalidQuery)?,
        attempt: row.get::<_, u32>(3)?,
        max_attempts: row.get::<_, u32>(4)?,
        scheduled_at: row.get(5)?,
        attempted_at: row.get(6)?,
    })
}
