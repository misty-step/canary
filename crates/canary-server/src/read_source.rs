//! Uniform read handle for authenticated read-model routes.
//!
//! Route handlers that only run `SELECT`-shaped queries call
//! [`IngestState::read_source`] instead of [`IngestState::lock_store`]. When
//! the runtime was wired with a [`ReadPool`] (the production boot path in
//! `runtime.rs`), the returned [`ReadSource`] serves queries from a
//! read-only WAL connection so read traffic no longer serializes behind the
//! single-writer mutex. When no pool was wired (every in-memory test store
//! in this crate), `ReadSource` falls back to the writer connection with
//! identical query results — this module is the only place that fallback
//! lives, so call sites never branch on it themselves.
//!
//! `report_error_groups_scoped` and `active_incidents` are deliberately not
//! mirrored here: `Store`'s versions fuse a claim-expiry write into the
//! read, so they always run on the writer. Callers needing them still use
//! `IngestState::lock_store`. That is the eligibility rule in general: a
//! read belongs here only if it performs no write at all. `active_claims`
//! qualifies because it is a pure predicate `SELECT` that deliberately
//! skips the expiry-stamping sweep the subject-scoped claim reads fuse in
//! (see `canary_store::claims::list_active`); a claims read that needs the
//! sweep must stay on the writer.

use parking_lot::MutexGuard;

use canary_core::query::{ActiveClaimsResponse, ErrorDetail, ErrorsByClass, TimelineResponse};
use canary_store::{
    ActiveClaimListOptions, ApiKeyRecord, ClaimResult, ErrorSummaryItem, HealthMonitorStatus,
    HealthTargetStatus, MetricsSnapshot, QueryResult, Result, Store, TargetCheckRead,
    TimelineQueryOptions, TimelineQueryResult,
};
use canary_store::{RecentTransition, SearchResult, ServiceSliSummary};

use crate::IngestState;

impl IngestState {
    /// Return a read handle for one request: a pooled read-only connection
    /// when a [`canary_store::ReadPool`] is wired, otherwise the writer
    /// store guard.
    pub(crate) fn read_source(&self) -> std::result::Result<ReadSource<'_>, String> {
        match self.read_pool() {
            Some(pool) => Ok(ReadSource::Pool(
                pool.checkout().map_err(|error| error.to_string())?,
            )),
            None => Ok(ReadSource::Writer(self.lock_store()?)),
        }
    }
}

impl ReadSource<'_> {
    /// Run `f` so every read-model query it issues observes one consistent
    /// snapshot. Callers whose block reads more than one row set that must
    /// agree with each other (a multi-section report, for example) must use
    /// this instead of calling query methods directly: a pool-backed source
    /// gives each bare query its own WAL snapshot, so two sequential
    /// queries could otherwise straddle a concurrent write and return a
    /// self-contradictory result. A writer-backed source is already atomic
    /// for the whole call because `IngestState::lock_store` holds the
    /// single-writer mutex for as long as the guard lives, so this is a
    /// plain call for that variant.
    pub(crate) fn with_snapshot<T>(&self, f: impl FnOnce() -> T) -> std::result::Result<T, String> {
        match self {
            Self::Pool(conn) => conn.with_snapshot(f).map_err(|error| error.to_string()),
            Self::Writer(_) => Ok(f()),
        }
    }
}

/// One request's read handle, backed by either a pooled read-only
/// connection or the single-writer store guard.
pub(crate) enum ReadSource<'a> {
    Pool(canary_store::ReadConnection),
    Writer(MutexGuard<'a, Store>),
}

impl ReadSource<'_> {
    pub(crate) fn health_targets_scoped(
        &self,
        tenant_id: &str,
        project_id: &str,
    ) -> Result<Vec<HealthTargetStatus>> {
        match self {
            Self::Pool(conn) => conn.health_targets_scoped(tenant_id, project_id),
            Self::Writer(store) => store.health_targets_scoped(tenant_id, project_id),
        }
    }

    pub(crate) fn health_monitors_scoped(
        &self,
        tenant_id: &str,
        project_id: &str,
    ) -> Result<Vec<HealthMonitorStatus>> {
        match self {
            Self::Pool(conn) => conn.health_monitors_scoped(tenant_id, project_id),
            Self::Writer(store) => store.health_monitors_scoped(tenant_id, project_id),
        }
    }

    pub(crate) fn target_checks_scoped(
        &self,
        target_id: &str,
        window: &str,
        tenant_id: &str,
        project_id: &str,
    ) -> QueryResult<Vec<TargetCheckRead>> {
        match self {
            Self::Pool(conn) => conn.target_checks_scoped(target_id, window, tenant_id, project_id),
            Self::Writer(store) => {
                store.target_checks_scoped(target_id, window, tenant_id, project_id)
            }
        }
    }

    pub(crate) fn target_checks_scoped_for_service(
        &self,
        target_id: &str,
        window: &str,
        tenant_id: &str,
        project_id: &str,
        service: &str,
    ) -> QueryResult<Vec<TargetCheckRead>> {
        match self {
            Self::Pool(conn) => conn.target_checks_scoped_for_service(
                target_id, window, tenant_id, project_id, service,
            ),
            Self::Writer(store) => store.target_checks_scoped_for_service(
                target_id, window, tenant_id, project_id, service,
            ),
        }
    }

    pub(crate) fn error_summary_scoped(
        &self,
        window: &str,
        tenant_id: &str,
        project_id: &str,
    ) -> QueryResult<Vec<ErrorSummaryItem>> {
        match self {
            Self::Pool(conn) => conn.error_summary_scoped(window, tenant_id, project_id),
            Self::Writer(store) => store.error_summary_scoped(window, tenant_id, project_id),
        }
    }

    pub(crate) fn errors_by_class_scoped(
        &self,
        window: &str,
        tenant_id: &str,
        project_id: &str,
    ) -> QueryResult<ErrorsByClass> {
        match self {
            Self::Pool(conn) => conn.errors_by_class_scoped(window, tenant_id, project_id),
            Self::Writer(store) => store.errors_by_class_scoped(window, tenant_id, project_id),
        }
    }

    pub(crate) fn error_detail_scoped(
        &self,
        error_id: &str,
        tenant_id: &str,
        project_id: &str,
    ) -> QueryResult<Option<ErrorDetail>> {
        match self {
            Self::Pool(conn) => conn.error_detail_scoped(error_id, tenant_id, project_id),
            Self::Writer(store) => store.error_detail_scoped(error_id, tenant_id, project_id),
        }
    }

    pub(crate) fn service_sli_scoped(
        &self,
        window: &str,
        tenant_id: &str,
        project_id: &str,
    ) -> QueryResult<Vec<ServiceSliSummary>> {
        match self {
            Self::Pool(conn) => conn.service_sli_scoped(window, tenant_id, project_id),
            Self::Writer(store) => store.service_sli_scoped(window, tenant_id, project_id),
        }
    }

    pub(crate) fn recent_transitions_scoped(
        &self,
        window: &str,
        tenant_id: &str,
        project_id: &str,
    ) -> QueryResult<Vec<RecentTransition>> {
        match self {
            Self::Pool(conn) => conn.recent_transitions_scoped(window, tenant_id, project_id),
            Self::Writer(store) => store.recent_transitions_scoped(window, tenant_id, project_id),
        }
    }

    pub(crate) fn search_errors_scoped(
        &self,
        query: &str,
        window: &str,
        tenant_id: &str,
        project_id: &str,
    ) -> QueryResult<Vec<SearchResult>> {
        match self {
            Self::Pool(conn) => conn.search_errors_scoped(query, window, tenant_id, project_id),
            Self::Writer(store) => store.search_errors_scoped(query, window, tenant_id, project_id),
        }
    }

    pub(crate) fn timeline(
        &self,
        window: &str,
        options: TimelineQueryOptions,
    ) -> TimelineQueryResult<TimelineResponse> {
        match self {
            Self::Pool(conn) => conn.timeline(window, options),
            Self::Writer(store) => store.timeline(window, options),
        }
    }

    pub(crate) fn list_api_keys_scoped(
        &self,
        tenant_id: &str,
        project_id: &str,
    ) -> Result<Vec<ApiKeyRecord>> {
        match self {
            Self::Pool(conn) => conn.list_api_keys_scoped(tenant_id, project_id),
            Self::Writer(store) => store.list_api_keys_scoped(tenant_id, project_id),
        }
    }

    pub(crate) fn active_claims(
        &self,
        options: &ActiveClaimListOptions,
    ) -> ClaimResult<ActiveClaimsResponse> {
        match self {
            Self::Pool(conn) => conn.active_claims(options),
            Self::Writer(store) => store.active_claims(options),
        }
    }

    pub(crate) fn metrics_snapshot(&self) -> Result<MetricsSnapshot> {
        match self {
            Self::Pool(conn) => conn.metrics_snapshot(),
            Self::Writer(store) => store.metrics_snapshot(),
        }
    }
}
