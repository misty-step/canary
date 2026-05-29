//! Retention prune persistence.

use rusqlite::{Connection, params};

use crate::Result;

const BATCH_SIZE: u32 = 1_000;

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

pub(crate) fn prune(
    connection: &mut Connection,
    prune: RetentionPrune,
) -> Result<RetentionPruneReport> {
    Ok(RetentionPruneReport {
        errors_deleted: prune_table(connection, "errors", "created_at", &prune.error_cutoff)?,
        service_events_deleted: prune_table(
            connection,
            "service_events",
            "created_at",
            &prune.error_cutoff,
        )?,
        target_checks_deleted: prune_table(
            connection,
            "target_checks",
            "checked_at",
            &prune.check_cutoff,
        )?,
    })
}

fn prune_table(connection: &Connection, table: &str, column: &str, cutoff: &str) -> Result<u64> {
    let sql = format!(
        "DELETE FROM {table}
         WHERE rowid IN (
             SELECT rowid FROM {table}
             WHERE {column} < ?1
             LIMIT ?2
         )"
    );
    let mut total = 0_u64;

    loop {
        let deleted = connection.execute(&sql, params![cutoff, BATCH_SIZE])? as u64;
        total += deleted;
        if deleted < u64::from(BATCH_SIZE) {
            break;
        }
    }

    Ok(total)
}
