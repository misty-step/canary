//! Read-only SQLite connections for concurrent read-model queries.
//!
//! [`Store`](crate::Store) keeps a single writable connection behind the
//! server's mutex (see the crate-level single-writer invariant). Read-model
//! routes historically ran on that same connection, so concurrent read
//! traffic serialized behind the writer mutex even though SQLite's WAL
//! journal mode already supports many concurrent readers against one file.
//!
//! `ReadPool` opens a fresh read-only connection per checkout against the
//! database file backing a `Store`. Opening a small WAL-mode connection is
//! cheap, and a per-checkout connection avoids pool-exhaustion and
//! checkout-deadlock failure modes entirely: there is no shared pool state
//! to contend on. `ReadPool` refuses to open a database that is not already
//! running in WAL journal mode, since a rollback-journal database still
//! serializes readers against the writer at the SQLite layer.
//!
//! In-memory stores (used throughout this crate's and canary-server's test
//! suites) have no second file to open a sibling connection against, so
//! `ReadPool` only supports file-backed databases. Callers that have not
//! wired a `ReadPool` keep reading through the writer connection exactly as
//! before; that fallback lives in canary-server's route state, not here.
//!
//! Deliberately absent: `report_error_groups_scoped`. `Store`'s version
//! fuses a claim-expiry write with the read (`&mut self`), and a read-only
//! connection cannot perform that write. Callers that need it stay on the
//! writer connection.

use std::path::{Path, PathBuf};
use std::time::Duration;

use canary_core::query::{ErrorDetail, ErrorsByClass};
use rusqlite::{Connection, OpenFlags};

use crate::{
    ApiKeyRecord, ErrorSummaryItem, HealthMonitorStatus, HealthTargetStatus, QueryResult, Result,
    ServiceSliSummary, StoreError, TargetCheckRead, TimelineQueryOptions, TimelineQueryResult,
};
use crate::{RecentTransition, SearchResult};
use crate::{api_keys, health, query, service_sli};

const BUSY_TIMEOUT: Duration = Duration::from_secs(5);

/// Opens read-only SQLite connections against the file backing a [`Store`](crate::Store).
#[derive(Clone)]
pub struct ReadPool {
    path: PathBuf,
}

impl ReadPool {
    /// Open a read pool against the database at `path`.
    ///
    /// Fails loudly if the database cannot be opened read-only or is not
    /// already running in WAL journal mode, instead of silently degrading to
    /// serialized readers.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        let probe = Self::connect(&path)?;
        let journal_mode: String =
            probe.pragma_query_value(None, "journal_mode", |row| row.get(0))?;
        if !journal_mode.eq_ignore_ascii_case("wal") {
            return Err(StoreError::ReadPoolNotWal(journal_mode));
        }
        Ok(Self { path })
    }

    fn connect(path: &Path) -> Result<Connection> {
        let connection = Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        connection.pragma_update(None, "query_only", true)?;
        connection.busy_timeout(BUSY_TIMEOUT)?;
        Ok(connection)
    }

    /// Check out a fresh read-only connection scoped to one request.
    pub fn checkout(&self) -> Result<ReadConnection> {
        Ok(ReadConnection {
            connection: Self::connect(&self.path)?,
        })
    }
}

/// One read-only SQLite connection checked out from a [`ReadPool`] for a
/// single request. Every method here mirrors the read-only subset of
/// [`Store`](crate::Store)'s query surface used by canary-server's
/// authenticated read routes.
pub struct ReadConnection {
    connection: Connection,
}

impl ReadConnection {
    /// Return target health rows for one tenant/project.
    pub fn health_targets_scoped(
        &self,
        tenant_id: &str,
        project_id: &str,
    ) -> Result<Vec<HealthTargetStatus>> {
        health::health_targets_scoped(&self.connection, tenant_id, project_id)
    }

    /// Return monitor health rows for one tenant/project.
    pub fn health_monitors_scoped(
        &self,
        tenant_id: &str,
        project_id: &str,
    ) -> Result<Vec<HealthMonitorStatus>> {
        health::health_monitors_scoped(&self.connection, tenant_id, project_id)
    }

    /// Query recent target checks when the target belongs to one tenant/project.
    pub fn target_checks_scoped(
        &self,
        target_id: &str,
        window: &str,
        tenant_id: &str,
        project_id: &str,
    ) -> QueryResult<Vec<TargetCheckRead>> {
        health::target_checks_scoped(&self.connection, target_id, window, tenant_id, project_id)
    }

    /// Query recent target checks when the target belongs to one service authority.
    pub fn target_checks_scoped_for_service(
        &self,
        target_id: &str,
        window: &str,
        tenant_id: &str,
        project_id: &str,
        service: &str,
    ) -> QueryResult<Vec<TargetCheckRead>> {
        health::target_checks_scoped_for_service(
            &self.connection,
            target_id,
            window,
            tenant_id,
            project_id,
            service,
        )
    }

    /// Query active error counts grouped by service for one tenant/project.
    pub fn error_summary_scoped(
        &self,
        window: &str,
        tenant_id: &str,
        project_id: &str,
    ) -> QueryResult<Vec<ErrorSummaryItem>> {
        query::error_summary_scoped(&self.connection, window, tenant_id, project_id)
    }

    /// Query recent error counts grouped by error class for one tenant/project.
    pub fn errors_by_class_scoped(
        &self,
        window: &str,
        tenant_id: &str,
        project_id: &str,
    ) -> QueryResult<ErrorsByClass> {
        query::errors_by_class_scoped(&self.connection, window, tenant_id, project_id)
    }

    /// Return one error detail read model for one tenant/project.
    pub fn error_detail_scoped(
        &self,
        error_id: &str,
        tenant_id: &str,
        project_id: &str,
    ) -> QueryResult<Option<ErrorDetail>> {
        query::error_detail_scoped(&self.connection, error_id, tenant_id, project_id)
    }

    /// Query windowed service SLI aggregates for one tenant/project.
    pub fn service_sli_scoped(
        &self,
        window: &str,
        tenant_id: &str,
        project_id: &str,
    ) -> QueryResult<Vec<ServiceSliSummary>> {
        service_sli::service_sli_scoped(&self.connection, window, tenant_id, project_id)
    }

    /// Query recent target and monitor transitions for one tenant/project.
    pub fn recent_transitions_scoped(
        &self,
        window: &str,
        tenant_id: &str,
        project_id: &str,
    ) -> QueryResult<Vec<RecentTransition>> {
        query::recent_transitions_scoped(&self.connection, window, tenant_id, project_id)
    }

    /// Search recent errors for one tenant/project.
    pub fn search_errors_scoped(
        &self,
        query_text: &str,
        window: &str,
        tenant_id: &str,
        project_id: &str,
    ) -> QueryResult<Vec<SearchResult>> {
        query::search_errors_scoped(&self.connection, query_text, window, tenant_id, project_id)
    }

    /// Query the durable service-event timeline.
    pub fn timeline(
        &self,
        window: &str,
        options: TimelineQueryOptions,
    ) -> TimelineQueryResult<canary_core::query::TimelineResponse> {
        query::timeline(&self.connection, window, options)
    }

    /// Return admin-visible API keys for one tenant/project.
    pub fn list_api_keys_scoped(
        &self,
        tenant_id: &str,
        project_id: &str,
    ) -> Result<Vec<ApiKeyRecord>> {
        api_keys::list_scoped(&self.connection, tenant_id, project_id)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::{BOOTSTRAP_PROJECT_ID, BOOTSTRAP_TENANT_ID, Store};
    use std::sync::{Arc, Barrier};
    use std::thread;

    fn temp_db_path(name: &str) -> PathBuf {
        let suffix = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |duration| duration.as_nanos());
        std::env::temp_dir().join(format!(
            "canary-read-pool-{name}-{}-{suffix}.db",
            std::process::id()
        ))
    }

    #[test]
    fn open_rejects_non_wal_database() -> Result<()> {
        let path = temp_db_path("non-wal");
        {
            let connection = Connection::open(&path)?;
            connection.pragma_update(None, "journal_mode", "DELETE")?;
        }

        let result = ReadPool::open(&path);

        let _ = std::fs::remove_file(&path);
        assert!(matches!(result, Err(StoreError::ReadPoolNotWal(_))));
        Ok(())
    }

    fn sample_target_insert(id: &str) -> crate::TargetInsert {
        crate::TargetInsert {
            id: id.to_owned(),
            url: "https://example.test/health".to_owned(),
            name: "read pool target".to_owned(),
            service: "canary".to_owned(),
            method: "GET".to_owned(),
            headers: None,
            interval_ms: 60_000,
            timeout_ms: 10_000,
            expected_status: "200".to_owned(),
            body_contains: None,
            degraded_after: 1,
            down_after: 3,
            up_after: 1,
            active: true,
            created_at: "2026-05-28T19:00:00Z".to_owned(),
        }
    }

    #[test]
    fn checkout_reads_committed_rows_without_touching_writer() -> Result<()> {
        let path = temp_db_path("reads-committed");
        let mut store = Store::open(&path)?;
        store.migrate()?;
        store.insert_target_scoped(
            sample_target_insert("TGT-read-pool-1"),
            BOOTSTRAP_TENANT_ID,
            BOOTSTRAP_PROJECT_ID,
        )?;
        drop(store);

        let pool = ReadPool::open(&path)?;
        let conn = pool.checkout()?;
        let targets = conn.health_targets_scoped(BOOTSTRAP_TENANT_ID, BOOTSTRAP_PROJECT_ID)?;

        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(format!("{}-wal", path.display()));
        let _ = std::fs::remove_file(format!("{}-shm", path.display()));

        assert_eq!(targets.len(), 1);
        assert_eq!(targets[0].id, "TGT-read-pool-1");
        Ok(())
    }

    /// A concurrent read via [`ReadPool`] must make progress while another
    /// thread holds an open SQLite write transaction on the writer
    /// connection, proving reads no longer serialize behind the writer.
    #[test]
    fn concurrent_read_makes_progress_while_writer_transaction_is_open() -> Result<()> {
        let path = temp_db_path("concurrent-progress");
        let mut store = Store::open(&path)?;
        store.migrate()?;
        store.insert_target_scoped(
            sample_target_insert("TGT-during-write"),
            BOOTSTRAP_TENANT_ID,
            BOOTSTRAP_PROJECT_ID,
        )?;

        let pool = ReadPool::open(&path)?;
        let writer_holding = Arc::new(Barrier::new(2));
        let release_writer = Arc::new(Barrier::new(2));

        let writer_thread = {
            let writer_holding = writer_holding.clone();
            let release_writer = release_writer.clone();
            thread::spawn(move || {
                store
                    .connection
                    .execute_batch("BEGIN IMMEDIATE")
                    .expect("begin writer transaction");
                writer_holding.wait();
                release_writer.wait();
                store
                    .connection
                    .execute_batch("COMMIT")
                    .expect("commit writer transaction");
            })
        };

        writer_holding.wait();
        let conn = pool.checkout()?;
        let started = std::time::Instant::now();
        let targets = conn.health_targets_scoped(BOOTSTRAP_TENANT_ID, BOOTSTRAP_PROJECT_ID)?;
        let elapsed = started.elapsed();
        release_writer.wait();
        writer_thread.join().expect("writer thread panicked");

        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(format!("{}-wal", path.display()));
        let _ = std::fs::remove_file(format!("{}-shm", path.display()));

        assert_eq!(targets.len(), 1);
        assert!(
            elapsed < Duration::from_millis(500),
            "read blocked on open writer transaction: took {elapsed:?}"
        );
        Ok(())
    }
}
