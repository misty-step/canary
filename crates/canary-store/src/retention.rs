//! Retention prune persistence.

use std::{path::Path, time::Duration};

use rusqlite::{Connection, OpenFlags, params};

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
    /// Terminal webhook delivery ledger rows.
    WebhookDeliveriesTerminal,
    /// Monitor check-ins, keyed by `observed_at`.
    MonitorCheckIns,
    /// Annotation audit rows, keyed by `created_at`.
    Annotations,
    /// Resolved incident signals, keyed by `resolved_at`.
    IncidentSignalsResolved,
    /// Terminal remediation claims.
    RemediationClaimsTerminal,
    /// Resolved incidents without an active remediation claim.
    IncidentsResolved,
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
    /// Deleted terminal webhook delivery ledger rows.
    pub webhook_deliveries_deleted: u64,
    /// Deleted monitor check-ins.
    pub monitor_check_ins_deleted: u64,
    /// Deleted annotations.
    pub annotations_deleted: u64,
    /// Deleted resolved incident signals.
    pub incident_signals_deleted: u64,
    /// Deleted terminal remediation claims.
    pub remediation_claims_deleted: u64,
    /// Deleted resolved incidents.
    pub incidents_deleted: u64,
    /// Deleted terminal webhook delivery jobs.
    pub oban_jobs_deleted: u64,
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

/// Page-level storage reclaimed by one bounded incremental vacuum.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct StorageReclaimReport {
    /// Pages on SQLite's freelist before the operation.
    pub freelist_pages_before: u64,
    /// Pages on SQLite's freelist after the operation.
    pub freelist_pages_after: u64,
    /// Pages returned to the filesystem.
    pub pages_reclaimed: u64,
}

/// Evidence emitted by the offline full-vacuum operator command.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VacuumDatabaseReport {
    /// Database pages before compaction.
    pub page_count_before: u64,
    /// Database pages after compaction.
    pub page_count_after: u64,
    /// Free pages before compaction.
    pub freelist_pages_before: u64,
    /// Free pages after compaction.
    pub freelist_pages_after: u64,
    /// SQLite page size in bytes.
    pub page_size: u64,
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
        webhook_deliveries_deleted: prune_table(
            connection,
            RetentionPruneTable::WebhookDeliveriesTerminal,
            &prune.error_cutoff,
        )?,
        monitor_check_ins_deleted: prune_table(
            connection,
            RetentionPruneTable::MonitorCheckIns,
            &prune.check_cutoff,
        )?,
        annotations_deleted: prune_table(
            connection,
            RetentionPruneTable::Annotations,
            &prune.error_cutoff,
        )?,
        incident_signals_deleted: prune_table(
            connection,
            RetentionPruneTable::IncidentSignalsResolved,
            &prune.error_cutoff,
        )?,
        remediation_claims_deleted: prune_table(
            connection,
            RetentionPruneTable::RemediationClaimsTerminal,
            &prune.error_cutoff,
        )?,
        incidents_deleted: prune_table(
            connection,
            RetentionPruneTable::IncidentsResolved,
            &prune.error_cutoff,
        )?,
        oban_jobs_deleted: prune_table(
            connection,
            RetentionPruneTable::ObanJobsTerminal,
            &prune.error_cutoff,
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

/// Reclaim at most 1,000 free pages after a retention pass.
pub(crate) fn incremental_vacuum(connection: &Connection) -> Result<StorageReclaimReport> {
    let freelist_pages_before = pragma_u64(connection, "freelist_count")?;
    connection.execute_batch("PRAGMA incremental_vacuum(1000)")?;
    let freelist_pages_after = pragma_u64(connection, "freelist_count")?;
    Ok(StorageReclaimReport {
        freelist_pages_before,
        freelist_pages_after,
        pages_reclaimed: freelist_pages_before.saturating_sub(freelist_pages_after),
    })
}

/// Compact one offline database and enable bounded incremental vacuuming for
/// subsequent retention passes. The open flags refuse to create a database
/// when the operator supplied the wrong path.
pub fn vacuum_database(path: impl AsRef<Path>) -> Result<VacuumDatabaseReport> {
    let connection = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    connection.busy_timeout(Duration::from_secs(5))?;
    connection.pragma_update(None, "foreign_keys", "ON")?;
    connection.pragma_update(None, "locking_mode", "EXCLUSIVE")?;
    connection.execute_batch("BEGIN EXCLUSIVE; COMMIT")?;

    let page_count_before = pragma_u64(&connection, "page_count")?;
    let freelist_pages_before = pragma_u64(&connection, "freelist_count")?;
    let page_size = pragma_u64(&connection, "page_size")?;

    connection.pragma_update(None, "auto_vacuum", "INCREMENTAL")?;
    connection.execute_batch("VACUUM; PRAGMA wal_checkpoint(TRUNCATE)")?;

    Ok(VacuumDatabaseReport {
        page_count_before,
        page_count_after: pragma_u64(&connection, "page_count")?,
        freelist_pages_before,
        freelist_pages_after: pragma_u64(&connection, "freelist_count")?,
        page_size,
    })
}

fn pragma_u64(connection: &Connection, pragma: &str) -> Result<u64> {
    Ok(connection.pragma_query_value(None, pragma, |row| row.get(0))?)
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
        RetentionPruneTable::WebhookDeliveriesTerminal => (
            "webhook_deliveries",
            "((status = 'delivered' AND delivered_at < ?1)
              OR (status = 'discarded' AND discarded_at < ?1)
              OR (status = 'suppressed' AND updated_at < ?1))",
        ),
        RetentionPruneTable::MonitorCheckIns => ("monitor_check_ins", "observed_at < ?1"),
        RetentionPruneTable::Annotations => ("annotations", "created_at < ?1"),
        RetentionPruneTable::IncidentSignalsResolved => (
            "incident_signals",
            "resolved_at IS NOT NULL AND resolved_at < ?1",
        ),
        RetentionPruneTable::RemediationClaimsTerminal => (
            "remediation_claims",
            "state IN ('verified', 'dismissed', 'expired', 'released')
              AND COALESCE(completed_at, released_at, updated_at) < ?1",
        ),
        RetentionPruneTable::IncidentsResolved => (
            "incidents",
            "state = 'resolved'
              AND resolved_at IS NOT NULL
              AND resolved_at < ?1
              AND NOT EXISTS (
                  SELECT 1 FROM remediation_claims
                  WHERE tenant_id = incidents.tenant_id
                    AND project_id = incidents.project_id
                    AND subject_type = 'incident'
                    AND subject_id = incidents.id
              )
              AND NOT EXISTS (
                  SELECT 1 FROM annotations
                  WHERE (
                      incident_id = incidents.id
                      OR (subject_type = 'incident' AND subject_id = incidents.id)
                    )
                    AND created_at >= ?1
              )
              AND NOT EXISTS (
                  SELECT 1 FROM incident_signals
                  WHERE incident_id = incidents.id
                    AND (resolved_at IS NULL OR resolved_at >= ?1)
              )",
        ),
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
