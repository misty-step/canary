//! Retention prune persistence.

use rusqlite::{Connection, params};

use crate::Result;

const BATCH_SIZE: u32 = 1_000;

/// Fixed retention tables owned by Canary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RetentionPruneTable {
    /// Error rows, keyed by `created_at`.
    Errors,
    /// Service event rows, keyed by `created_at`.
    ServiceEvents,
    /// Target check rows, keyed by `checked_at`.
    TargetChecks,
    /// Terminal (`completed`/`discarded`) webhook-delivery job rows, keyed by
    /// their terminal timestamp. Non-terminal rows (`available`, `scheduled`,
    /// `executing`) are never matched, so claimed work is never pruned.
    ObanJobsTerminal,
}

/// Cutoff timestamps for one retention prune pass.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RetentionPrune {
    /// Delete errors and service events older than this timestamp.
    pub error_cutoff: String,
    /// Delete target checks older than this timestamp.
    pub check_cutoff: String,
}

/// Number of rows deleted by a retention prune pass.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RetentionPruneReport {
    /// Deleted error rows.
    pub errors_deleted: u64,
    /// Deleted service-event rows.
    pub service_events_deleted: u64,
    /// Deleted target-check rows.
    pub target_checks_deleted: u64,
}

/// One bounded retention delete statement.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RetentionPruneBatch {
    /// Table to prune.
    pub table: RetentionPruneTable,
    /// Delete rows older than this timestamp.
    pub cutoff: String,
}

/// Result of one bounded retention delete statement.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RetentionPruneBatchReport {
    /// Deleted rows.
    pub deleted: u64,
    /// True when this table had fewer old rows than one batch.
    pub complete: bool,
}

pub(crate) fn prune(
    connection: &mut Connection,
    prune: RetentionPrune,
) -> Result<RetentionPruneReport> {
    Ok(RetentionPruneReport {
        errors_deleted: prune_table(connection, RetentionPruneTable::Errors, &prune.error_cutoff)?,
        service_events_deleted: prune_table(
            connection,
            RetentionPruneTable::ServiceEvents,
            &prune.error_cutoff,
        )?,
        target_checks_deleted: prune_table(
            connection,
            RetentionPruneTable::TargetChecks,
            &prune.check_cutoff,
        )?,
    })
}

pub(crate) fn prune_batch(
    connection: &Connection,
    batch: RetentionPruneBatch,
) -> Result<RetentionPruneBatchReport> {
    let (table, predicate) = table_predicate(batch.table);
    let sql = format!(
        "DELETE FROM {table}
         WHERE rowid IN (
             SELECT rowid FROM {table}
             WHERE {predicate}
             LIMIT ?2
         )"
    );
    let deleted = connection.execute(&sql, params![batch.cutoff, BATCH_SIZE])? as u64;

    Ok(RetentionPruneBatchReport {
        deleted,
        complete: deleted < u64::from(BATCH_SIZE),
    })
}

fn prune_table(connection: &Connection, table: RetentionPruneTable, cutoff: &str) -> Result<u64> {
    let mut total = 0_u64;

    loop {
        let report = prune_batch(
            connection,
            RetentionPruneBatch {
                table,
                cutoff: cutoff.to_owned(),
            },
        )?;
        total += report.deleted;
        if report.complete {
            break;
        }
    }

    Ok(total)
}

/// Table name and cutoff predicate (bound to `?1`) for one bounded delete batch.
fn table_predicate(table: RetentionPruneTable) -> (&'static str, &'static str) {
    match table {
        RetentionPruneTable::Errors => ("errors", "created_at < ?1"),
        RetentionPruneTable::ServiceEvents => ("service_events", "created_at < ?1"),
        RetentionPruneTable::TargetChecks => ("target_checks", "checked_at < ?1"),
        // Only terminal states are eligible. `available`/`scheduled`/`executing`
        // rows never match this predicate regardless of cutoff. `cancelled` is
        // a legacy Elixir-era state the Rust write path never produces, but
        // pre-cutover rows may carry it; it is terminal and prunes like the
        // others.
        RetentionPruneTable::ObanJobsTerminal => (
            "oban_jobs",
            "((state = 'completed' AND completed_at < ?1)
              OR (state = 'discarded' AND discarded_at < ?1)
              OR (state = 'cancelled' AND cancelled_at < ?1))",
        ),
    }
}
