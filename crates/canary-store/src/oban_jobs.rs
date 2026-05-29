//! Minimal Oban-compatible persistence for Rust-owned webhook delivery jobs.

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
    /// Phoenix-compatible job args.
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
    /// Phoenix-compatible args.
    pub args: Value,
    /// Current one-based attempt after claiming.
    pub attempt: u32,
    /// Maximum attempts before final discard.
    pub max_attempts: u32,
    /// Scheduled timestamp.
    pub scheduled_at: String,
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
    let ids = {
        let mut statement = transaction.prepare(
            "SELECT id
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
                row.get::<_, i64>(0)
            })?;
        rows.collect::<std::result::Result<Vec<_>, _>>()?
    };

    for id in &ids {
        transaction.execute(
            "UPDATE oban_jobs
             SET state = 'executing',
                 attempt = attempt + 1,
                 attempted_at = ?2,
                 attempted_by = '[\"canary-rust\"]'
             WHERE id = ?1",
            params![id, now],
        )?;
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
    job_id: i64,
    completion: WebhookDeliveryJobCompletion,
) -> Result<()> {
    match completion {
        WebhookDeliveryJobCompletion::Retry { scheduled_at } => {
            connection.execute(
                "UPDATE oban_jobs
                 SET state = 'scheduled', scheduled_at = ?2
                 WHERE id = ?1",
                params![job_id, scheduled_at],
            )?;
        }
        WebhookDeliveryJobCompletion::Complete { now } => {
            connection.execute(
                "UPDATE oban_jobs
                 SET state = 'completed', completed_at = ?2
                 WHERE id = ?1",
                params![job_id, now],
            )?;
        }
        WebhookDeliveryJobCompletion::Discard { now } => {
            connection.execute(
                "UPDATE oban_jobs
                 SET state = 'discarded', discarded_at = ?2
                 WHERE id = ?1",
                params![job_id, now],
            )?;
        }
    }
    Ok(())
}

pub(crate) fn webhook_delivery_job(
    connection: &Connection,
    job_id: i64,
) -> Result<Option<WebhookDeliveryJobRow>> {
    let mut statement = connection.prepare(
        "SELECT id, state, args, attempt, max_attempts, scheduled_at
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
    })
}
