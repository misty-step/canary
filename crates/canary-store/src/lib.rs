//! SQLite persistence boundary for the Rust rewrite of Canary.
//!
//! This crate owns the database shape and the single-writer connection. Callers
//! ask it to migrate or persist product operations; they do not assemble SQL
//! from HTTP handlers or worker code.

use std::path::Path;

use rusqlite::Connection;

mod annotations;
mod api_keys;
mod health;
mod incidents;
mod ingest;
mod metrics;
mod oban_jobs;
mod query;
mod retention;
mod schema;
mod webhook_deliveries;

pub use annotations::{
    AnnotationError, AnnotationInsert, AnnotationPageOptions, AnnotationResult,
    AnnotationSubjectType, subject_types as annotation_subject_types,
};
pub use api_keys::{API_KEY_PREFIX_LEN, ApiKeyInsert, ApiKeyRecord, VerifiedApiKey};
pub use canary_core::metrics::MetricsSnapshot;
pub use health::{
    ActiveTargetProbeSchedule, HealthCheckSummary, HealthMonitorStatus, HealthTargetStatus,
    HealthTransitionCommit, MonitorCheckInCommit, MonitorCheckInCommitResult,
    MonitorCheckInObservation, MonitorCheckInSnapshot, MonitorInsert, MonitorOverdueCandidate,
    MonitorOverdueCommit, MonitorOverdueCommitResult, MonitorRecord, MonitorTransitionEvent,
    TargetCheckObservation, TargetCheckRead, TargetConflict, TargetInsert, TargetIntervalUpdate,
    TargetProbeCommit, TargetProbeCommitResult, TargetProbeSnapshot, TargetRecord,
    TargetTransitionEvent, TlsExpiryEventCommit, TlsExpiryEventInsert, TlsExpiryScanCandidate,
};
pub use incidents::{IncidentCorrelation, IncidentCorrelationEvent};
pub use ingest::{
    ErrorIngest, ErrorIngestCommit, ErrorIngestIds, ErrorIngestPayload, ErrorServiceEvent,
};
pub use oban_jobs::{
    WebhookDeliveryJobCompletion, WebhookDeliveryJobInsert, WebhookDeliveryJobRow,
    WebhookDeliveryJobState,
};
pub use query::{
    ErrorSummaryItem, IncidentListOptions, QueryError, QueryResult, RecentTransition, SearchResult,
    ServiceQueryOptions, TimelineQueryError, TimelineQueryOptions, TimelineQueryResult,
};
pub use retention::{
    RetentionPrune, RetentionPruneBatch, RetentionPruneBatchReport, RetentionPruneReport,
    RetentionPruneTable,
};
pub use webhook_deliveries::{
    WebhookDeliveryInsert, WebhookDeliveryListOptions, WebhookDeliveryPageError,
    WebhookDeliveryPageOptions, WebhookDeliveryPageResult, WebhookDeliveryRow,
    WebhookDeliveryStatus, WebhookSubscription, WebhookSubscriptionInsert,
    statuses as webhook_delivery_statuses,
};

/// Result type returned by the store boundary.
pub type Result<T> = std::result::Result<T, StoreError>;

/// Persistence-layer failure.
#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    /// SQLite rejected a connection, pragma, migration, or query.
    #[error("sqlite error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    /// A service-onboarding target conflicts with an existing target row.
    #[error("target conflict")]
    TargetConflict(TargetConflict),
}

/// Canary's single writable SQLite connection.
///
/// The Phoenix service deliberately runs `Canary.Repo` with `pool_size: 1`.
/// The Rust rewrite keeps the same operational invariant by making writes go
/// through this owned connection instead of exposing a generic connection pool.
pub struct Store {
    connection: Connection,
}

impl Store {
    /// Open a writable SQLite database at `path`.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        Self::from_connection(Connection::open(path)?)
    }

    /// Open a writable in-memory SQLite database.
    pub fn open_in_memory() -> Result<Self> {
        Self::from_connection(Connection::open_in_memory()?)
    }

    /// Apply Canary's ordered schema migrations.
    pub fn migrate(&mut self) -> Result<()> {
        schema::migrate(&mut self.connection)?;
        Ok(())
    }

    /// Return the Rust schema version stored in `PRAGMA user_version`.
    pub fn schema_version(&self) -> Result<u32> {
        let version = self
            .connection
            .query_row("PRAGMA user_version", [], |row| row.get::<_, u32>(0))?;
        Ok(version)
    }

    /// Commit one validated error ingest transaction.
    pub fn commit_error_ingest(&mut self, ingest: ErrorIngest) -> Result<ErrorIngestCommit> {
        ingest::commit(&mut self.connection, ingest)
    }

    /// Correlate one post-commit signal into Canary's incident graph.
    pub fn correlate_incident(
        &mut self,
        correlation: IncidentCorrelation,
    ) -> Result<Option<IncidentCorrelationEvent>> {
        incidents::correlate(&mut self.connection, correlation)
    }

    /// Persist one target probe, including state and optional transition effects.
    pub fn commit_target_probe(
        &mut self,
        probe: TargetProbeCommit,
    ) -> Result<TargetProbeCommitResult> {
        health::commit_target_probe(&mut self.connection, probe)
    }

    /// Insert one HTTP target row.
    pub fn insert_target(&mut self, target: TargetInsert) -> Result<()> {
        health::insert_target(&self.connection, target)
    }

    /// Create one service-onboarding target and ingest key as one product unit.
    pub fn commit_service_onboarding_target_and_key(
        &mut self,
        target: TargetInsert,
        key: ApiKeyInsert,
    ) -> Result<()> {
        let transaction = self.connection.transaction()?;
        let conflict = health::target_conflict(&transaction, &target.service, &target.url)?;
        if conflict.service || conflict.url {
            return Err(StoreError::TargetConflict(conflict));
        }
        health::insert_target(&transaction, target)?;
        api_keys::insert(&transaction, key)?;
        transaction.commit()?;
        Ok(())
    }

    /// Return admin-visible target rows ordered by display name.
    pub fn list_targets(&self) -> Result<Vec<TargetRecord>> {
        health::list_targets(&self.connection)
    }

    /// Return read-model target rows for health-status endpoints.
    pub fn health_targets(&self) -> Result<Vec<HealthTargetStatus>> {
        health::health_targets(&self.connection)
    }

    /// Query recent target checks for one target.
    pub fn target_checks(
        &self,
        target_id: &str,
        window: &str,
    ) -> QueryResult<Vec<TargetCheckRead>> {
        health::target_checks(&self.connection, target_id, window)
    }

    /// Delete one target row.
    pub fn delete_target(&mut self, target_id: &str) -> Result<bool> {
        health::delete_target(&mut self.connection, target_id)
    }

    /// Update one target's active flag.
    pub fn update_target_active(&mut self, target_id: &str, active: bool) -> Result<bool> {
        health::update_target_active(&mut self.connection, target_id, active)
    }

    /// Update one target's probe interval and return the previous cadence context.
    pub fn update_target_interval(
        &mut self,
        target_id: &str,
        interval_ms: i64,
    ) -> Result<Option<TargetIntervalUpdate>> {
        health::update_target_interval(&mut self.connection, target_id, interval_ms)
    }

    /// Return one active target configuration and state snapshot by id.
    ///
    /// If the target exists but has no state row yet, this method creates the
    /// Phoenix-compatible `unknown` state while the single-writer store lock is
    /// held by the caller.
    pub fn target_probe_snapshot_by_id(
        &mut self,
        target_id: &str,
    ) -> Result<Option<TargetProbeSnapshot>> {
        health::target_probe_snapshot_by_id(&mut self.connection, target_id)
    }

    /// Return active target ids and intervals for the probe lifecycle adapter.
    pub fn active_target_probe_schedules(&self) -> Result<Vec<ActiveTargetProbeSchedule>> {
        health::active_target_probe_schedules(&self.connection)
    }

    /// Persist one monitor check-in, including state and optional transition effects.
    pub fn commit_monitor_check_in(
        &mut self,
        check_in: MonitorCheckInCommit,
    ) -> Result<MonitorCheckInCommitResult> {
        health::commit_monitor_check_in(&mut self.connection, check_in)
    }

    /// Persist one overdue monitor transition without inserting a check-in row.
    pub fn commit_monitor_overdue(
        &mut self,
        overdue: MonitorOverdueCommit,
    ) -> Result<MonitorOverdueCommitResult> {
        health::commit_monitor_overdue(&mut self.connection, overdue)
    }

    /// Insert one non-HTTP monitor row.
    pub fn insert_monitor(&mut self, monitor: MonitorInsert) -> Result<()> {
        health::insert_monitor(&self.connection, monitor)
    }

    /// Create one non-HTTP monitor and its initial unknown state row.
    pub fn create_monitor(&mut self, monitor: MonitorInsert) -> Result<bool> {
        health::create_monitor(&mut self.connection, monitor)
    }

    /// Return admin-visible monitor rows ordered by name.
    pub fn list_monitors(&self) -> Result<Vec<MonitorRecord>> {
        health::list_monitors(&self.connection)
    }

    /// Return read-model monitor rows for health-status endpoints.
    pub fn health_monitors(&self) -> Result<Vec<HealthMonitorStatus>> {
        health::health_monitors(&self.connection)
    }

    /// Delete one non-HTTP monitor row.
    pub fn delete_monitor(&mut self, monitor_id: &str) -> Result<bool> {
        health::delete_monitor(&mut self.connection, monitor_id)
    }

    /// Return one monitor configuration and state snapshot by check-in name.
    ///
    /// If the monitor exists but has no state row yet, this method creates the
    /// Phoenix-compatible `unknown` state while the single-writer store lock is
    /// held by the caller.
    pub fn monitor_check_in_snapshot_by_name(
        &mut self,
        name: &str,
    ) -> Result<Option<MonitorCheckInSnapshot>> {
        health::monitor_check_in_snapshot_by_name(&mut self.connection, name)
    }

    /// Return monitor state rows that have deadlines eligible for overdue evaluation.
    pub fn monitor_overdue_candidates(&self) -> Result<Vec<MonitorOverdueCandidate>> {
        health::monitor_overdue_candidates(&self.connection)
    }

    /// Return active HTTPS targets with their latest persisted TLS expiry.
    pub fn tls_expiry_scan_candidates(&self) -> Result<Vec<TlsExpiryScanCandidate>> {
        health::tls_expiry_scan_candidates(&self.connection)
    }

    /// Persist one TLS-expiring service event for post-commit webhook fanout.
    pub fn record_tls_expiring_event(
        &mut self,
        event: TlsExpiryEventInsert,
    ) -> Result<TlsExpiryEventCommit> {
        health::record_tls_expiring_event(&mut self.connection, event)
    }

    /// Insert one API-key row whose raw secret has already been bcrypt-hashed.
    pub fn insert_api_key(&mut self, key: ApiKeyInsert) -> Result<()> {
        api_keys::insert(&self.connection, key)
    }

    /// Return admin-visible API-key rows ordered newest first.
    pub fn list_api_keys(&self) -> Result<Vec<ApiKeyRecord>> {
        api_keys::list(&self.connection)
    }

    /// Revoke one API key by id.
    pub fn revoke_api_key(&mut self, key_id: &str, revoked_at: &str) -> Result<bool> {
        api_keys::revoke(&self.connection, key_id, revoked_at)
    }

    /// Verify a raw bearer token against active bcrypt-hashed API-key rows.
    pub fn verify_api_key(&self, raw_key: &str) -> Result<Option<VerifiedApiKey>> {
        api_keys::verify_key(&self.connection, raw_key)
    }

    /// Query recent error groups for a service.
    pub fn errors_by_service(
        &self,
        service: &str,
        window: &str,
        options: ServiceQueryOptions,
    ) -> QueryResult<canary_core::query::ErrorsByService> {
        query::errors_by_service(&self.connection, service, window, options)
    }

    /// Query recent error groups for a service at a deterministic evaluation time.
    pub fn errors_by_service_at(
        &self,
        service: &str,
        window: &str,
        options: ServiceQueryOptions,
        now: time::OffsetDateTime,
    ) -> QueryResult<canary_core::query::ErrorsByService> {
        query::errors_by_service_at(&self.connection, service, window, options, now)
    }

    /// Query recent error groups for an error class.
    pub fn errors_by_error_class(
        &self,
        error_class: &str,
        window: &str,
        service: Option<&str>,
        options: ServiceQueryOptions,
    ) -> QueryResult<canary_core::query::ErrorsByErrorClass> {
        query::errors_by_error_class(&self.connection, error_class, window, service, options)
    }

    /// Query recent error groups for an error class at a deterministic evaluation time.
    pub fn errors_by_error_class_at(
        &self,
        error_class: &str,
        window: &str,
        service: Option<&str>,
        options: ServiceQueryOptions,
        now: time::OffsetDateTime,
    ) -> QueryResult<canary_core::query::ErrorsByErrorClass> {
        query::errors_by_error_class_at(
            &self.connection,
            error_class,
            window,
            service,
            options,
            now,
        )
    }

    /// Query recent error counts grouped by error class.
    pub fn errors_by_class(&self, window: &str) -> QueryResult<canary_core::query::ErrorsByClass> {
        query::errors_by_class(&self.connection, window)
    }

    /// Query active error counts grouped by service for combined status.
    pub fn error_summary(&self, window: &str) -> QueryResult<Vec<ErrorSummaryItem>> {
        query::error_summary(&self.connection, window)
    }

    /// Query active error groups for the unified report.
    pub fn report_error_groups(
        &self,
        window: &str,
    ) -> QueryResult<Vec<canary_core::query::ErrorGroupSummary>> {
        query::report_error_groups(&self.connection, window)
    }

    /// Query active error groups for the unified report at a deterministic time.
    pub fn report_error_groups_at(
        &self,
        window: &str,
        now: time::OffsetDateTime,
    ) -> QueryResult<Vec<canary_core::query::ErrorGroupSummary>> {
        query::report_error_groups_at(&self.connection, window, now)
    }

    /// Query recent target and monitor transitions for the unified report.
    pub fn recent_transitions(&self, window: &str) -> QueryResult<Vec<RecentTransition>> {
        query::recent_transitions(&self.connection, window)
    }

    /// Search recent errors for the unified report.
    pub fn search_errors(&self, query: &str, window: &str) -> QueryResult<Vec<SearchResult>> {
        query::search_errors(&self.connection, query, window)
    }

    /// Query the durable service-event timeline.
    pub fn timeline(
        &self,
        window: &str,
        options: TimelineQueryOptions,
    ) -> TimelineQueryResult<canary_core::query::TimelineResponse> {
        query::timeline(&self.connection, window, options)
    }

    /// Query recent error counts grouped by error class at a deterministic evaluation time.
    pub fn errors_by_class_at(
        &self,
        window: &str,
        now: time::OffsetDateTime,
    ) -> QueryResult<canary_core::query::ErrorsByClass> {
        query::errors_by_class_at(&self.connection, window, now)
    }

    /// Query active incidents with currently active signals.
    pub fn active_incidents(
        &self,
        options: IncidentListOptions,
    ) -> QueryResult<canary_core::query::ActiveIncidents> {
        query::active_incidents(&self.connection, options)
    }

    /// Query active incidents with currently active signals at a deterministic evaluation time.
    pub fn active_incidents_at(
        &self,
        options: IncidentListOptions,
        now: time::OffsetDateTime,
    ) -> QueryResult<canary_core::query::ActiveIncidents> {
        query::active_incidents_at(&self.connection, options, now)
    }

    /// Return one incident detail read model.
    pub fn incident_detail(
        &self,
        incident_id: &str,
    ) -> QueryResult<Option<canary_core::query::IncidentDetail>> {
        query::incident_detail(&self.connection, incident_id)
    }

    /// Return one error detail read model.
    pub fn error_detail(
        &self,
        error_id: &str,
    ) -> QueryResult<Option<canary_core::query::ErrorDetail>> {
        query::error_detail(&self.connection, error_id)
    }

    /// Create one annotation after verifying the target subject exists.
    pub fn create_annotation(
        &mut self,
        insert: AnnotationInsert,
    ) -> AnnotationResult<canary_core::query::Annotation> {
        annotations::create(&self.connection, insert)
    }

    /// List annotations for legacy incident and error-group routes.
    pub fn annotations(
        &self,
        subject_type: &str,
        subject_id: &str,
    ) -> AnnotationResult<canary_core::query::AnnotationListResponse> {
        annotations::list(&self.connection, subject_type, subject_id)
    }

    /// Page annotations for the unified read route.
    pub fn annotation_page(
        &self,
        options: AnnotationPageOptions,
    ) -> AnnotationResult<canary_core::query::AnnotationPageResponse> {
        annotations::page(&self.connection, options)
    }

    /// Insert a pending webhook delivery ledger row.
    pub fn create_pending_webhook_delivery(
        &mut self,
        delivery: WebhookDeliveryInsert,
    ) -> Result<()> {
        webhook_deliveries::create_pending(&mut self.connection, delivery)
    }

    /// Insert or update a suppressed webhook delivery ledger row.
    pub fn create_suppressed_webhook_delivery(
        &mut self,
        delivery: WebhookDeliveryInsert,
        reason: &str,
    ) -> Result<()> {
        webhook_deliveries::create_suppressed(&mut self.connection, delivery, reason)
    }

    /// Mark one webhook delivery attempt.
    pub fn mark_webhook_delivery_attempt(&mut self, delivery_id: &str, now: &str) -> Result<()> {
        webhook_deliveries::mark_attempt(&mut self.connection, delivery_id, now)
    }

    /// Mark one webhook delivery as delivered if the ledger row exists.
    pub fn mark_webhook_delivery_delivered(&mut self, delivery_id: &str, now: &str) -> Result<()> {
        webhook_deliveries::mark_delivered(&mut self.connection, delivery_id, now)
    }

    /// Mark one webhook delivery as discarded if the ledger row exists.
    pub fn mark_webhook_delivery_discarded(
        &mut self,
        delivery_id: &str,
        reason: &str,
        now: &str,
    ) -> Result<()> {
        webhook_deliveries::mark_discarded(&mut self.connection, delivery_id, reason, now)
    }

    /// List webhook delivery ledger rows in Phoenix's deterministic order.
    pub fn webhook_deliveries(
        &self,
        options: WebhookDeliveryListOptions,
    ) -> Result<Vec<WebhookDeliveryRow>> {
        webhook_deliveries::list(&self.connection, options)
    }

    /// Page through webhook delivery ledger rows for the public read API.
    pub fn webhook_delivery_page(
        &self,
        options: WebhookDeliveryPageOptions,
    ) -> WebhookDeliveryPageResult<canary_core::query::WebhookDeliveriesResponse> {
        webhook_deliveries::page(&self.connection, options)
    }

    /// Return active webhook subscriptions for one event.
    pub fn active_webhook_subscriptions_for_event(
        &self,
        event: &str,
    ) -> Result<Vec<WebhookSubscription>> {
        webhook_deliveries::active_subscriptions_for_event(&self.connection, event)
    }

    /// Return one webhook subscription by id, including inactive rows.
    pub fn webhook_subscription(&self, webhook_id: &str) -> Result<Option<WebhookSubscription>> {
        webhook_deliveries::subscription_by_id(&self.connection, webhook_id)
    }

    /// Return all webhook subscriptions in admin list order.
    pub fn webhook_subscriptions(&self) -> Result<Vec<WebhookSubscription>> {
        webhook_deliveries::list_subscriptions(&self.connection)
    }

    /// Insert one webhook subscription row.
    pub fn insert_webhook_subscription(
        &mut self,
        subscription: WebhookSubscriptionInsert,
    ) -> Result<()> {
        webhook_deliveries::insert_subscription(&mut self.connection, subscription)
    }

    /// Delete one webhook subscription row.
    pub fn delete_webhook_subscription(&mut self, webhook_id: &str) -> Result<bool> {
        webhook_deliveries::delete_subscription(&mut self.connection, webhook_id)
    }

    /// Insert one scheduled webhook delivery job.
    pub fn insert_webhook_delivery_job(&mut self, job: WebhookDeliveryJobInsert) -> Result<i64> {
        oban_jobs::insert_webhook_delivery_job(&mut self.connection, job)
    }

    /// Claim due webhook delivery jobs and increment their attempt counters.
    pub fn claim_due_webhook_delivery_jobs(
        &mut self,
        now: &str,
        limit: u32,
    ) -> Result<Vec<WebhookDeliveryJobRow>> {
        oban_jobs::claim_due_webhook_delivery_jobs(&mut self.connection, now, limit)
    }

    /// Apply one scheduler-side completion transition for a claimed webhook job.
    pub fn complete_webhook_delivery_job(
        &mut self,
        job_id: i64,
        completion: WebhookDeliveryJobCompletion,
    ) -> Result<()> {
        oban_jobs::complete_webhook_delivery_job(&mut self.connection, job_id, completion)
    }

    /// Return one webhook delivery job row by id.
    pub fn webhook_delivery_job(&self, job_id: i64) -> Result<Option<WebhookDeliveryJobRow>> {
        oban_jobs::webhook_delivery_job(&self.connection, job_id)
    }

    /// Gather a point-in-time Prometheus metrics snapshot.
    pub fn metrics_snapshot(&self) -> Result<MetricsSnapshot> {
        metrics::snapshot(&self.connection)
    }

    /// Prune old errors, service events, and target checks in bounded batches.
    pub fn prune_retention(&mut self, prune: RetentionPrune) -> Result<RetentionPruneReport> {
        retention::prune(&mut self.connection, prune)
    }

    /// Execute one bounded retention prune statement.
    pub fn prune_retention_batch(
        &mut self,
        batch: RetentionPruneBatch,
    ) -> Result<RetentionPruneBatchReport> {
        retention::prune_batch(&self.connection, batch)
    }

    /// Count persisted errors.
    pub fn error_count(&self) -> Result<u64> {
        let count = self
            .connection
            .query_row("SELECT count(*) FROM errors", [], |row| {
                row.get::<_, u64>(0)
            })?;
        Ok(count)
    }

    fn from_connection(connection: Connection) -> Result<Self> {
        connection.pragma_update(None, "foreign_keys", "ON")?;
        connection.pragma_update(None, "journal_mode", "WAL")?;
        connection.busy_timeout(std::time::Duration::from_secs(5))?;
        Ok(Self { connection })
    }
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, BTreeSet};
    use std::str::FromStr;

    use canary_core::{
        health::state_machine::HealthState,
        ids::{ErrorId, EventId, IncidentId},
        ingest::classification::{Category, Classification, Component, Persistence},
    };
    use rusqlite::params;
    use serde_json::{Value, json};
    use time::{OffsetDateTime, format_description::well_known::Rfc3339};

    use super::*;

    #[derive(Debug, PartialEq, Eq)]
    struct Column {
        name: String,
        data_type: String,
        not_null: bool,
        default_value: Option<String>,
        primary_key_position: i64,
    }

    #[test]
    fn migrate_creates_the_current_phoenix_tables() -> Result<()> {
        let mut store = migrated_store()?;

        assert_eq!(
            table_names(&store.connection)?,
            BTreeSet::from([
                "annotations".to_owned(),
                "api_keys".to_owned(),
                "error_groups".to_owned(),
                "errors".to_owned(),
                "errors_fts".to_owned(),
                "errors_fts_config".to_owned(),
                "errors_fts_data".to_owned(),
                "errors_fts_docsize".to_owned(),
                "errors_fts_idx".to_owned(),
                "incident_signals".to_owned(),
                "incidents".to_owned(),
                "monitor_check_ins".to_owned(),
                "monitor_state".to_owned(),
                "monitors".to_owned(),
                "oban_jobs".to_owned(),
                "seed_runs".to_owned(),
                "service_events".to_owned(),
                "target_checks".to_owned(),
                "target_state".to_owned(),
                "targets".to_owned(),
                "webhook_deliveries".to_owned(),
                "webhooks".to_owned(),
            ])
        );
        assert_eq!(store.schema_version()?, schema::SCHEMA_VERSION);

        store.migrate()?;
        assert_eq!(store.schema_version()?, schema::SCHEMA_VERSION);

        Ok(())
    }

    #[test]
    fn errors_table_matches_required_columns_defaults_and_classification() -> Result<()> {
        let store = migrated_store()?;
        let columns = columns(&store.connection, "errors")?;

        assert_column(
            &columns,
            "id",
            ColumnSpec::new("TEXT").primary_key_position(1),
        );
        assert_column(&columns, "service", ColumnSpec::new("TEXT").not_null());
        assert_column(&columns, "error_class", ColumnSpec::new("TEXT").not_null());
        assert_column(&columns, "message", ColumnSpec::new("TEXT").not_null());
        assert_column(&columns, "message_template", ColumnSpec::new("TEXT"));
        assert_column(&columns, "stack_trace", ColumnSpec::new("TEXT"));
        assert_column(&columns, "context", ColumnSpec::new("TEXT"));
        assert_column(
            &columns,
            "severity",
            ColumnSpec::new("TEXT").default_value("'error'"),
        );
        assert_column(
            &columns,
            "environment",
            ColumnSpec::new("TEXT").default_value("'production'"),
        );
        assert_column(&columns, "group_hash", ColumnSpec::new("TEXT").not_null());
        assert_column(&columns, "fingerprint", ColumnSpec::new("TEXT"));
        assert_column(&columns, "region", ColumnSpec::new("TEXT"));
        assert_column(&columns, "created_at", ColumnSpec::new("TEXT").not_null());
        assert_column(&columns, "classification_category", ColumnSpec::new("TEXT"));
        assert_column(
            &columns,
            "classification_persistence",
            ColumnSpec::new("TEXT"),
        );
        assert_column(
            &columns,
            "classification_component",
            ColumnSpec::new("TEXT"),
        );
        assert_indexes(
            &store.connection,
            "errors",
            &[
                "errors_service_created_at_index",
                "errors_group_hash_created_at_index",
            ],
        )?;

        Ok(())
    }

    #[test]
    fn fts_table_and_triggers_track_error_rows() -> Result<()> {
        let store = migrated_store()?;

        store.connection.execute(
            "INSERT INTO errors (
                id, service, error_class, message, stack_trace, group_hash, created_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                "ERR-123456789abc",
                "sploot",
                "RuntimeError",
                "boom in worker",
                "stack line",
                "group-a",
                "2026-05-28T20:00:00Z"
            ],
        )?;

        let count = store.connection.query_row(
            "SELECT count(*) FROM errors_fts WHERE errors_fts MATCH 'worker'",
            [],
            |row| row.get::<_, i64>(0),
        )?;

        assert_eq!(count, 1);
        assert_eq!(
            trigger_names(&store.connection, "errors")?,
            BTreeSet::from([
                "errors_fts_delete".to_owned(),
                "errors_fts_insert".to_owned(),
                "errors_fts_update".to_owned(),
            ])
        );

        Ok(())
    }

    #[test]
    fn incidents_keep_one_open_incident_per_service() -> Result<()> {
        let store = migrated_store()?;

        store.connection.execute(
            "INSERT INTO incidents (id, service, state, severity, opened_at)
             VALUES ('INC-123456789abc', 'sploot', 'investigating', 'medium', '2026-05-28T20:00:00Z')",
            [],
        )?;
        let duplicate = store.connection.execute(
            "INSERT INTO incidents (id, service, state, severity, opened_at)
             VALUES ('INC-abcdefghijkl', 'sploot', 'investigating', 'medium', '2026-05-28T20:01:00Z')",
            [],
        );

        assert!(duplicate.is_err());

        Ok(())
    }

    #[test]
    fn webhook_delivery_ledger_preserves_primary_key_and_indexes() -> Result<()> {
        let store = migrated_store()?;
        let columns = columns(&store.connection, "webhook_deliveries")?;

        assert_column(
            &columns,
            "delivery_id",
            ColumnSpec::new("TEXT").primary_key_position(1),
        );
        assert_column(&columns, "webhook_id", ColumnSpec::new("TEXT").not_null());
        assert_column(&columns, "event", ColumnSpec::new("TEXT").not_null());
        assert_column(&columns, "status", ColumnSpec::new("TEXT").not_null());
        assert_column(
            &columns,
            "attempt_count",
            ColumnSpec::new("INTEGER").not_null().default_value("0"),
        );
        assert_column(&columns, "created_at", ColumnSpec::new("TEXT").not_null());
        assert_column(&columns, "updated_at", ColumnSpec::new("TEXT").not_null());
        assert_indexes(
            &store.connection,
            "webhook_deliveries",
            &[
                "webhook_deliveries_created_at_delivery_id_index",
                "webhook_deliveries_webhook_id_created_at_delivery_id_index",
                "webhook_deliveries_event_created_at_delivery_id_index",
                "webhook_deliveries_status_created_at_delivery_id_index",
            ],
        )?;

        Ok(())
    }

    #[test]
    fn webhook_delivery_ledger_tracks_attempt_success_and_final_discard() -> Result<()> {
        let mut store = migrated_store()?;

        store.create_pending_webhook_delivery(WebhookDeliveryInsert {
            delivery_id: "DLV-123456789abc".to_owned(),
            webhook_id: "WHK-123456789abc".to_owned(),
            event: "error.new_class".to_owned(),
            now: "2026-05-28T20:00:00Z".to_owned(),
        })?;
        store.mark_webhook_delivery_attempt("DLV-123456789abc", "2026-05-28T20:00:01Z")?;
        store.mark_webhook_delivery_attempt("DLV-123456789abc", "2026-05-28T20:00:02Z")?;

        let rows = store.webhook_deliveries(WebhookDeliveryListOptions {
            delivery_id: Some("DLV-123456789abc".to_owned()),
            ..WebhookDeliveryListOptions::default()
        })?;
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].status, WebhookDeliveryStatus::Retrying);
        assert_eq!(rows[0].attempt_count, 2);
        assert_eq!(
            rows[0].first_attempt_at.as_deref(),
            Some("2026-05-28T20:00:01Z")
        );
        assert_eq!(
            rows[0].last_attempt_at.as_deref(),
            Some("2026-05-28T20:00:02Z")
        );

        store.mark_webhook_delivery_delivered("DLV-123456789abc", "2026-05-28T20:00:03Z")?;
        let delivered = store.webhook_deliveries(WebhookDeliveryListOptions {
            status: Some(WebhookDeliveryStatus::Delivered),
            ..WebhookDeliveryListOptions::default()
        })?;
        assert_eq!(delivered.len(), 1);
        assert_eq!(
            delivered[0].delivered_at.as_deref(),
            Some("2026-05-28T20:00:03Z")
        );

        store.create_pending_webhook_delivery(WebhookDeliveryInsert {
            delivery_id: "DLV-abcdefghijkl".to_owned(),
            webhook_id: "WHK-123456789abc".to_owned(),
            event: "error.new_class".to_owned(),
            now: "2026-05-28T20:01:00Z".to_owned(),
        })?;
        store.mark_webhook_delivery_attempt("DLV-abcdefghijkl", "2026-05-28T20:01:01Z")?;
        store.mark_webhook_delivery_discarded(
            "DLV-abcdefghijkl",
            "http_500",
            "2026-05-28T20:01:02Z",
        )?;
        let discarded = store.webhook_deliveries(WebhookDeliveryListOptions {
            status: Some(WebhookDeliveryStatus::Discarded),
            ..WebhookDeliveryListOptions::default()
        })?;
        assert_eq!(discarded.len(), 1);
        assert_eq!(discarded[0].reason.as_deref(), Some("http_500"));
        assert_eq!(
            discarded[0].discarded_at.as_deref(),
            Some("2026-05-28T20:01:02Z")
        );

        Ok(())
    }

    #[test]
    fn webhook_delivery_suppression_is_idempotent_and_queryable() -> Result<()> {
        let mut store = migrated_store()?;

        store.create_pending_webhook_delivery(WebhookDeliveryInsert {
            delivery_id: "DLV-123456789abc".to_owned(),
            webhook_id: "WHK-123456789abc".to_owned(),
            event: "error.new_class".to_owned(),
            now: "2026-05-28T20:00:00Z".to_owned(),
        })?;
        store.create_suppressed_webhook_delivery(
            WebhookDeliveryInsert {
                delivery_id: "DLV-123456789abc".to_owned(),
                webhook_id: "WHK-123456789abc".to_owned(),
                event: "error.new_class".to_owned(),
                now: "2026-05-28T20:00:05Z".to_owned(),
            },
            "cooldown",
        )?;

        let rows = store.webhook_deliveries(WebhookDeliveryListOptions {
            webhook_id: Some("WHK-123456789abc".to_owned()),
            event: Some("error.new_class".to_owned()),
            status: Some(WebhookDeliveryStatus::Suppressed),
            ..WebhookDeliveryListOptions::default()
        })?;
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].delivery_id, "DLV-123456789abc");
        assert_eq!(rows[0].reason.as_deref(), Some("cooldown"));
        assert_eq!(rows[0].attempt_count, 0);

        Ok(())
    }

    #[test]
    fn webhook_delivery_page_filters_paginates_and_rejects_invalid_inputs()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let mut store = migrated_store()?;
        for (delivery_id, webhook_id, event, now) in [
            (
                "DLV-a",
                "WHK-alpha",
                "error.new_class",
                "2026-05-28T20:00:00Z",
            ),
            (
                "DLV-c",
                "WHK-beta",
                "incident.updated",
                "2026-05-28T20:02:00Z",
            ),
            (
                "DLV-b",
                "WHK-alpha",
                "error.new_class",
                "2026-05-28T20:01:00Z",
            ),
            (
                "DLV-d",
                "WHK-alpha",
                "error.new_class",
                "2026-05-28T20:01:00Z",
            ),
        ] {
            store.create_suppressed_webhook_delivery(
                WebhookDeliveryInsert {
                    delivery_id: delivery_id.to_owned(),
                    webhook_id: webhook_id.to_owned(),
                    event: event.to_owned(),
                    now: now.to_owned(),
                },
                "cooldown",
            )?;
        }

        let first = store.webhook_delivery_page(WebhookDeliveryPageOptions {
            status: Some("suppressed".to_owned()),
            limit: Some("2".to_owned()),
            ..Default::default()
        })?;
        assert_eq!(first.returned_count, 2);
        assert_eq!(first.deliveries[0].delivery_id, "DLV-c");
        assert_eq!(first.deliveries[1].delivery_id, "DLV-d");
        assert_eq!(
            first.deliveries[0].completed_at.as_deref(),
            Some("2026-05-28T20:02:00Z")
        );
        let cursor = first.cursor.ok_or("expected next cursor")?;

        let second = store.webhook_delivery_page(WebhookDeliveryPageOptions {
            status: Some("suppressed".to_owned()),
            limit: Some("1".to_owned()),
            cursor: Some(cursor),
            ..Default::default()
        })?;
        assert_eq!(second.returned_count, 1);
        assert_eq!(second.deliveries[0].delivery_id, "DLV-b");
        let cursor = second.cursor.ok_or("expected same-timestamp cursor")?;

        let third = store.webhook_delivery_page(WebhookDeliveryPageOptions {
            status: Some("suppressed".to_owned()),
            limit: Some("2".to_owned()),
            cursor: Some(cursor),
            ..Default::default()
        })?;
        assert_eq!(third.returned_count, 1);
        assert_eq!(third.deliveries[0].delivery_id, "DLV-a");
        assert_eq!(third.cursor, None);

        let filtered = store.webhook_delivery_page(WebhookDeliveryPageOptions {
            webhook_id: Some("WHK-alpha".to_owned()),
            event: Some("error.new_class".to_owned()),
            status: Some("suppressed".to_owned()),
            ..Default::default()
        })?;
        assert_eq!(filtered.returned_count, 3);
        assert!(
            filtered
                .deliveries
                .iter()
                .all(|delivery| delivery.webhook_id == "WHK-alpha")
        );

        assert!(matches!(
            store.webhook_delivery_page(WebhookDeliveryPageOptions {
                limit: Some("0".to_owned()),
                ..Default::default()
            }),
            Err(WebhookDeliveryPageError::InvalidLimit)
        ));
        assert!(matches!(
            store.webhook_delivery_page(WebhookDeliveryPageOptions {
                cursor: Some("bogus".to_owned()),
                ..Default::default()
            }),
            Err(WebhookDeliveryPageError::InvalidCursor)
        ));
        assert!(matches!(
            store.webhook_delivery_page(WebhookDeliveryPageOptions {
                status: Some("supressed".to_owned()),
                ..Default::default()
            }),
            Err(WebhookDeliveryPageError::InvalidStatus)
        ));

        Ok(())
    }

    #[test]
    fn annotation_page_creates_lists_paginates_and_rejects_invalid_inputs()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let mut store = migrated_store()?;
        store.insert_target(TargetInsert {
            id: "TGT-api".to_owned(),
            url: "https://api.example.com/health".to_owned(),
            name: "api".to_owned(),
            service: "api".to_owned(),
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
        })?;

        for (id, agent, created_at) in [
            ("ANN-a", "alpha", "2026-05-28T20:00:00Z"),
            ("ANN-b", "beta", "2026-05-28T20:00:01Z"),
            ("ANN-c", "gamma", "2026-05-28T20:00:01Z"),
        ] {
            store.create_annotation(AnnotationInsert {
                id: id.to_owned(),
                subject_type: "target".to_owned(),
                subject_id: "TGT-api".to_owned(),
                agent: agent.to_owned(),
                action: "acknowledged".to_owned(),
                metadata: Some(json!({"agent": agent})),
                created_at: created_at.to_owned(),
            })?;
        }

        let first = store.annotation_page(AnnotationPageOptions {
            subject_type: "target".to_owned(),
            subject_id: "TGT-api".to_owned(),
            limit: Some("2".to_owned()),
            cursor: None,
        })?;
        assert_eq!(
            first
                .annotations
                .iter()
                .map(|annotation| annotation.agent.as_str())
                .collect::<Vec<_>>(),
            ["gamma", "beta"]
        );
        assert!(first.summary.contains("3 annotations"));
        assert!(
            first
                .summary
                .contains("latest from gamma at 2026-05-28T20:00:01Z")
        );
        let cursor = first.cursor.ok_or("expected annotation cursor")?;

        let second = store.annotation_page(AnnotationPageOptions {
            subject_type: "target".to_owned(),
            subject_id: "TGT-api".to_owned(),
            limit: Some("2".to_owned()),
            cursor: Some(cursor),
        })?;
        assert_eq!(second.annotations.len(), 1);
        assert_eq!(second.annotations[0].agent, "alpha");
        assert_eq!(second.cursor, None);
        assert!(
            second
                .summary
                .contains("latest from gamma at 2026-05-28T20:00:01Z")
        );

        let legacy = store.annotations("target", "TGT-api")?;
        assert_eq!(legacy.annotations.len(), 3);
        assert_eq!(
            legacy.annotations[0].metadata,
            Some(json!({"agent": "gamma"}))
        );

        assert!(matches!(
            store.annotation_page(AnnotationPageOptions {
                subject_type: "target".to_owned(),
                subject_id: "TGT-api".to_owned(),
                limit: Some("0".to_owned()),
                cursor: None,
            }),
            Err(AnnotationError::InvalidLimit)
        ));
        assert!(matches!(
            store.annotation_page(AnnotationPageOptions {
                subject_type: "target".to_owned(),
                subject_id: "TGT-api".to_owned(),
                limit: Some("51".to_owned()),
                cursor: None,
            }),
            Err(AnnotationError::InvalidLimit)
        ));
        assert!(matches!(
            store.annotation_page(AnnotationPageOptions {
                subject_type: "target".to_owned(),
                subject_id: "TGT-api".to_owned(),
                limit: None,
                cursor: Some("bogus".to_owned()),
            }),
            Err(AnnotationError::InvalidCursor)
        ));
        assert!(matches!(
            store.annotation_page(AnnotationPageOptions {
                subject_type: "spaceship".to_owned(),
                subject_id: "X-1".to_owned(),
                limit: None,
                cursor: None,
            }),
            Err(AnnotationError::InvalidSubjectType)
        ));
        assert!(matches!(
            store.annotation_page(AnnotationPageOptions {
                subject_type: "target".to_owned(),
                subject_id: "TGT-missing".to_owned(),
                limit: None,
                cursor: None,
            }),
            Err(AnnotationError::NotFound)
        ));

        Ok(())
    }

    #[test]
    fn webhook_delivery_jobs_claim_due_rows_once_and_increment_attempt() -> Result<()> {
        let mut store = migrated_store()?;
        let due_job = store.insert_webhook_delivery_job(WebhookDeliveryJobInsert {
            args: json!({
                "webhook_id": "WHK-test",
                "payload": {"sequence": 7},
                "event": "error.new_class",
                "delivery_id": "DLV-due"
            }),
            scheduled_at: "2026-05-28T20:00:00Z".to_owned(),
            now: "2026-05-28T20:00:00Z".to_owned(),
            max_attempts: 4,
        })?;
        store.insert_webhook_delivery_job(WebhookDeliveryJobInsert {
            args: json!({
                "webhook_id": "WHK-test",
                "payload": {"sequence": 8},
                "event": "error.new_class",
                "delivery_id": "DLV-future"
            }),
            scheduled_at: "2026-05-28T20:10:00Z".to_owned(),
            now: "2026-05-28T20:00:00Z".to_owned(),
            max_attempts: 4,
        })?;

        let claimed = store.claim_due_webhook_delivery_jobs("2026-05-28T20:00:01Z", 10)?;

        assert_eq!(claimed.len(), 1);
        assert_eq!(claimed[0].id, due_job);
        assert_eq!(claimed[0].state, WebhookDeliveryJobState::Executing);
        assert_eq!(claimed[0].attempt, 1);
        assert_eq!(claimed[0].max_attempts, 4);
        assert_eq!(claimed[0].args["delivery_id"], "DLV-due");
        assert!(
            store
                .claim_due_webhook_delivery_jobs("2026-05-28T20:00:01Z", 10)?
                .is_empty()
        );

        Ok(())
    }

    #[test]
    fn webhook_delivery_jobs_complete_reschedule_and_discard_claimed_rows() -> Result<()> {
        let mut store = migrated_store()?;
        let job = store.insert_webhook_delivery_job(WebhookDeliveryJobInsert {
            args: json!({
                "webhook_id": "WHK-test",
                "payload": {"sequence": 7},
                "event": "error.new_class",
                "delivery_id": "DLV-retry"
            }),
            scheduled_at: "2026-05-28T20:00:00Z".to_owned(),
            now: "2026-05-28T20:00:00Z".to_owned(),
            max_attempts: 4,
        })?;
        assert_eq!(
            store
                .claim_due_webhook_delivery_jobs("2026-05-28T20:00:00Z", 1)?
                .len(),
            1
        );

        store.complete_webhook_delivery_job(
            job,
            WebhookDeliveryJobCompletion::Retry {
                scheduled_at: "2026-05-28T20:00:06Z".to_owned(),
            },
        )?;
        let retry = store
            .webhook_delivery_job(job)?
            .ok_or(StoreError::Sqlite(rusqlite::Error::QueryReturnedNoRows))?;
        assert_eq!(retry.state, WebhookDeliveryJobState::Scheduled);
        assert_eq!(retry.scheduled_at, "2026-05-28T20:00:06Z");
        assert_eq!(retry.attempt, 1);
        assert!(
            store
                .claim_due_webhook_delivery_jobs("2026-05-28T20:00:05Z", 1)?
                .is_empty()
        );

        let claimed_again = store.claim_due_webhook_delivery_jobs("2026-05-28T20:00:06Z", 1)?;
        assert_eq!(claimed_again[0].attempt, 2);
        store.complete_webhook_delivery_job(
            job,
            WebhookDeliveryJobCompletion::Complete {
                now: "2026-05-28T20:00:07Z".to_owned(),
            },
        )?;
        assert_eq!(
            store
                .webhook_delivery_job(job)?
                .ok_or(StoreError::Sqlite(rusqlite::Error::QueryReturnedNoRows))?
                .state,
            WebhookDeliveryJobState::Completed
        );

        let discarded = store.insert_webhook_delivery_job(WebhookDeliveryJobInsert {
            args: json!({
                "webhook_id": "WHK-test",
                "payload": {"sequence": 9},
                "event": "error.new_class",
                "delivery_id": "DLV-discard"
            }),
            scheduled_at: "2026-05-28T20:01:00Z".to_owned(),
            now: "2026-05-28T20:01:00Z".to_owned(),
            max_attempts: 1,
        })?;
        store.complete_webhook_delivery_job(
            discarded,
            WebhookDeliveryJobCompletion::Discard {
                now: "2026-05-28T20:01:01Z".to_owned(),
            },
        )?;
        assert_eq!(
            store
                .webhook_delivery_job(discarded)?
                .ok_or(StoreError::Sqlite(rusqlite::Error::QueryReturnedNoRows))?
                .state,
            WebhookDeliveryJobState::Discarded
        );

        Ok(())
    }

    #[test]
    fn active_webhook_subscriptions_filter_by_active_flag_and_event() -> Result<()> {
        let store = migrated_store()?;

        insert_webhook(
            &store,
            "WHK-123456789abc",
            "[\"error.new_class\",\"health_check.state_change\"]",
            1,
            "2026-05-28T20:00:00Z",
        )?;
        insert_webhook(
            &store,
            "WHK-abcdefghijkl",
            "[\"annotation.added\"]",
            1,
            "2026-05-28T20:01:00Z",
        )?;
        insert_webhook(
            &store,
            "WHK-inactive000",
            "[\"error.new_class\"]",
            0,
            "2026-05-28T20:02:00Z",
        )?;

        let subscriptions = store.active_webhook_subscriptions_for_event("error.new_class")?;
        assert_eq!(subscriptions.len(), 1);
        assert_eq!(subscriptions[0].id, "WHK-123456789abc");
        assert!(subscriptions[0].active);
        assert!(subscriptions[0].subscribes_to("health_check.state_change"));

        Ok(())
    }

    #[test]
    fn admin_webhook_subscriptions_list_insert_and_delete_rows() -> Result<()> {
        let mut store = migrated_store()?;
        insert_webhook(
            &store,
            "WHK-zeta",
            "[\"annotation.added\"]",
            1,
            "2026-05-28T20:02:00Z",
        )?;
        store.insert_webhook_subscription(WebhookSubscriptionInsert {
            id: "WHK-alpha".to_owned(),
            url: "https://example.test/alpha".to_owned(),
            events: vec!["error.new_class".to_owned(), "canary.ping".to_owned()],
            secret: "generated-secret".to_owned(),
            active: true,
            created_at: "2026-05-28T20:00:00Z".to_owned(),
        })?;

        let subscriptions = store.webhook_subscriptions()?;
        assert_eq!(subscriptions.len(), 2);
        assert_eq!(subscriptions[0].id, "WHK-alpha");
        assert_eq!(subscriptions[0].url, "https://example.test/alpha");
        assert_eq!(
            subscriptions[0].events,
            "[\"error.new_class\",\"canary.ping\"]"
        );
        assert!(subscriptions[0].active);
        assert_eq!(subscriptions[1].id, "WHK-zeta");

        assert!(store.delete_webhook_subscription("WHK-alpha")?);
        assert!(store.webhook_subscription("WHK-alpha")?.is_none());
        assert!(!store.delete_webhook_subscription("WHK-alpha")?);

        Ok(())
    }

    #[test]
    fn webhook_subscription_lookup_returns_inactive_rows_for_executor() -> Result<()> {
        let store = migrated_store()?;
        insert_webhook(
            &store,
            "WHK-inactive000",
            "[\"error.new_class\"]",
            0,
            "2026-05-28T20:02:00Z",
        )?;

        let subscription = store
            .webhook_subscription("WHK-inactive000")?
            .ok_or_else(|| StoreError::Sqlite(rusqlite::Error::QueryReturnedNoRows))?;

        assert_eq!(subscription.id, "WHK-inactive000");
        assert!(!subscription.active);
        assert!(subscription.subscribes_to("error.new_class"));
        assert!(store.webhook_subscription("WHK-missing")?.is_none());

        Ok(())
    }

    #[test]
    fn correlate_incident_opens_error_group_incident_and_records_timeline() -> Result<()> {
        let mut store = migrated_store()?;
        store.commit_error_ingest(error_ingest(
            "ERR-123456789abc",
            "EVT-123456789abc",
            "group-incident-a",
            "2026-05-28T20:00:00Z",
        ))?;

        let event = store
            .correlate_incident(incident_correlation(
                "INC-123456789abc",
                "EVT-abcdefghijkl",
                "error_group",
                "group-incident-a",
                "cadence",
                "2026-05-28T20:00:05Z",
            ))?
            .ok_or(StoreError::Sqlite(rusqlite::Error::QueryReturnedNoRows))?;

        assert_eq!(event.event, "incident.opened");
        assert_eq!(event.id, "EVT-abcdefghijkl");
        let payload: Value =
            serde_json::from_str(&event.payload_json).map_err(|_| rusqlite::Error::InvalidQuery)?;
        assert_eq!(payload["event"], "incident.opened");
        assert_eq!(payload["incident"]["id"], "INC-123456789abc");
        assert_eq!(
            payload["incident"]["signals"][0]["signal_ref"],
            "group-incident-a"
        );

        let incident = incident_row(&store, "INC-123456789abc")?;
        assert_eq!(
            incident,
            (
                "cadence".to_owned(),
                "investigating".to_owned(),
                "medium".to_owned(),
                None
            )
        );
        assert_eq!(
            signal_count(
                &store,
                "INC-123456789abc",
                "error_group",
                "group-incident-a"
            )?,
            1
        );
        assert_eq!(
            service_event_count(&store.connection, "INC-123456789abc", "incident.opened")?,
            1
        );

        Ok(())
    }

    #[test]
    fn correlate_incident_updates_existing_incident_and_escalates_after_three_signals() -> Result<()>
    {
        let mut store = migrated_store()?;
        for (index, group_hash) in ["group-signal-a", "group-signal-b", "group-signal-c"]
            .iter()
            .enumerate()
        {
            store.commit_error_ingest(error_ingest(
                &format!("ERR-signal{index}abc"),
                &format!("EVT-signal{index}abc"),
                group_hash,
                "2026-05-28T20:00:00Z",
            ))?;
        }

        assert_eq!(
            store
                .correlate_incident(incident_correlation(
                    "INC-abcdefghijkl",
                    "EVT-incident001",
                    "error_group",
                    "group-signal-a",
                    "cadence",
                    "2026-05-28T20:00:01Z",
                ))?
                .map(|event| event.event),
            Some("incident.opened".to_owned())
        );
        assert_eq!(
            store
                .correlate_incident(incident_correlation(
                    "INC-unused0000",
                    "EVT-incident002",
                    "error_group",
                    "group-signal-b",
                    "cadence",
                    "2026-05-28T20:00:02Z",
                ))?
                .map(|event| event.event),
            Some("incident.updated".to_owned())
        );
        assert_eq!(
            store
                .correlate_incident(incident_correlation(
                    "INC-unused0001",
                    "EVT-incident003",
                    "error_group",
                    "group-signal-c",
                    "cadence",
                    "2026-05-28T20:00:03Z",
                ))?
                .map(|event| event.event),
            Some("incident.updated".to_owned())
        );

        let incident = incident_row(&store, "INC-abcdefghijkl")?;
        assert_eq!(incident.2, "high");
        assert_eq!(active_signal_count(&store, "INC-abcdefghijkl")?, 3);

        Ok(())
    }

    #[test]
    fn correlate_incident_resolves_when_attached_error_group_is_no_longer_active() -> Result<()> {
        let mut store = migrated_store()?;
        store.commit_error_ingest(error_ingest(
            "ERR-123456789abc",
            "EVT-123456789abc",
            "group-resolve",
            "2026-05-28T20:00:00Z",
        ))?;
        store.correlate_incident(incident_correlation(
            "INC-resolve00000",
            "EVT-resolve001",
            "error_group",
            "group-resolve",
            "cadence",
            "2026-05-28T20:00:01Z",
        ))?;
        store.connection.execute(
            "UPDATE error_groups SET status = 'resolved' WHERE group_hash = 'group-resolve'",
            [],
        )?;

        let event = store
            .correlate_incident(incident_correlation(
                "INC-unused0002",
                "EVT-resolve002",
                "error_group",
                "group-resolve",
                "cadence",
                "2026-05-28T20:00:02Z",
            ))?
            .ok_or(StoreError::Sqlite(rusqlite::Error::QueryReturnedNoRows))?;

        assert_eq!(event.event, "incident.resolved");
        let incident = incident_row(&store, "INC-resolve00000")?;
        assert_eq!(incident.1, "resolved");
        assert_eq!(incident.3.as_deref(), Some("2026-05-28T20:00:02Z"));
        assert_eq!(active_signal_count(&store, "INC-resolve00000")?, 0);

        Ok(())
    }

    #[test]
    fn correlate_incident_ignores_inactive_signal_without_open_incident() -> Result<()> {
        let mut store = migrated_store()?;
        store.commit_error_ingest(error_ingest(
            "ERR-123456789abc",
            "EVT-123456789abc",
            "group-stale",
            "2026-05-28T20:00:00Z",
        ))?;

        let event = store.correlate_incident(incident_correlation(
            "INC-stale00000",
            "EVT-stale00000",
            "error_group",
            "group-stale",
            "cadence",
            "2026-05-28T20:10:00Z",
        ))?;

        assert_eq!(event, None);
        assert_eq!(row_count(&store.connection, "incidents")?, 0);
        Ok(())
    }

    #[test]
    fn correlate_incident_uses_typed_health_state_activity_contract() -> Result<()> {
        let mut store = migrated_store()?;
        store.insert_target(TargetInsert {
            id: "TGT-paused".to_owned(),
            url: "https://api.example.com".to_owned(),
            name: "Paused API".to_owned(),
            service: "api".to_owned(),
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
        })?;
        store.connection.execute(
            "INSERT INTO target_state (target_id, state) VALUES ('TGT-paused', 'paused')",
            [],
        )?;

        let opened = store
            .correlate_incident(incident_correlation(
                "INC-paused000000",
                "EVT-pausedopen",
                "health_transition",
                "TGT-paused",
                "api",
                "2026-05-28T20:00:00Z",
            ))?
            .ok_or(StoreError::Sqlite(rusqlite::Error::QueryReturnedNoRows))?;
        assert_eq!(opened.event, "incident.opened");
        assert_eq!(active_signal_count(&store, "INC-paused000000")?, 1);

        store.connection.execute(
            "UPDATE target_state SET state = 'up' WHERE target_id = 'TGT-paused'",
            [],
        )?;
        let resolved = store
            .correlate_incident(incident_correlation(
                "INC-unused0003",
                "EVT-pausedup00",
                "health_transition",
                "TGT-paused",
                "api",
                "2026-05-28T20:01:00Z",
            ))?
            .ok_or(StoreError::Sqlite(rusqlite::Error::QueryReturnedNoRows))?;

        assert_eq!(resolved.event, "incident.resolved");
        assert_eq!(incident_row(&store, "INC-paused000000")?.1, "resolved");
        assert_eq!(active_signal_count(&store, "INC-paused000000")?, 0);

        Ok(())
    }

    #[test]
    fn api_keys_table_preserves_phoenix_hash_storage_shape() -> Result<()> {
        let store = migrated_store()?;
        let columns = columns(&store.connection, "api_keys")?;

        assert_column(
            &columns,
            "id",
            ColumnSpec::new("TEXT").primary_key_position(1),
        );
        assert_column(&columns, "name", ColumnSpec::new("TEXT").not_null());
        assert_column(&columns, "key_prefix", ColumnSpec::new("TEXT").not_null());
        assert_column(&columns, "key_hash", ColumnSpec::new("TEXT").not_null());
        assert_column(&columns, "created_at", ColumnSpec::new("TEXT").not_null());
        assert_column(&columns, "revoked_at", ColumnSpec::new("TEXT"));
        assert_column(
            &columns,
            "scope",
            ColumnSpec::new("TEXT").not_null().default_value("'admin'"),
        );

        Ok(())
    }

    #[test]
    fn verify_api_key_matches_active_bcrypt_candidate()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let mut store = migrated_store()?;
        insert_api_key(
            &mut store,
            "KEY-valid",
            "sk_live_valid_secret",
            "ingest-only",
            None,
        )?;

        let Some(verified) = store.verify_api_key("sk_live_valid_secret")? else {
            return Err("key should verify".into());
        };

        assert_eq!(verified.id, "KEY-valid");
        assert_eq!(verified.name, "key KEY-valid");
        assert_eq!(verified.scope, "ingest-only");
        Ok(())
    }

    #[test]
    fn verify_api_key_rejects_wrong_raw_key_with_matching_prefix()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let mut store = migrated_store()?;
        insert_api_key(
            &mut store,
            "KEY-prefix",
            "sk_live_same_secret",
            "admin",
            None,
        )?;

        let verified = store.verify_api_key("sk_live_same_wrong")?;

        assert_eq!(verified, None);
        Ok(())
    }

    #[test]
    fn verify_api_key_checks_all_same_prefix_candidates()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let mut store = migrated_store()?;
        insert_api_key(&mut store, "KEY-first", "sk_live_same_first", "admin", None)?;
        insert_api_key(
            &mut store,
            "KEY-second",
            "sk_live_same_second",
            "read-only",
            None,
        )?;

        let Some(verified) = store.verify_api_key("sk_live_same_second")? else {
            return Err("second same-prefix key should verify".into());
        };

        assert_eq!(verified.id, "KEY-second");
        assert_eq!(verified.scope, "read-only");
        Ok(())
    }

    #[test]
    fn verify_api_key_rejects_revoked_and_unknown_prefix_keys()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let mut store = migrated_store()?;
        insert_api_key(
            &mut store,
            "KEY-revoked",
            "sk_live_revoked_secret",
            "ingest-only",
            Some("2026-05-28T20:05:00Z"),
        )?;

        assert_eq!(store.verify_api_key("sk_live_revoked_secret")?, None);
        assert_eq!(store.verify_api_key("sk_live_missing_secret")?, None);
        Ok(())
    }

    #[test]
    fn admin_api_keys_list_and_revoke_metadata_without_hashes()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let mut store = migrated_store()?;
        insert_api_key(
            &mut store,
            "KEY-old",
            "sk_live_old_secret",
            "read-only",
            None,
        )?;
        store.insert_api_key(ApiKeyInsert {
            id: "KEY-new".to_owned(),
            name: "deploy key".to_owned(),
            key_prefix: api_keys::key_prefix("sk_live_new_secret"),
            key_hash: bcrypt::hash("sk_live_new_secret", bcrypt::DEFAULT_COST)?,
            created_at: "2026-05-28T21:00:00Z".to_owned(),
            revoked_at: None,
            scope: "admin".to_owned(),
        })?;

        let keys = store.list_api_keys()?;

        assert_eq!(keys.len(), 2);
        assert_eq!(keys[0].id, "KEY-new");
        assert_eq!(keys[0].name, "deploy key");
        assert_eq!(keys[0].scope, "admin");
        assert_eq!(keys[0].key_prefix, "sk_live_new_");
        assert_eq!(keys[0].revoked_at, None);
        assert_eq!(keys[1].id, "KEY-old");

        assert!(store.revoke_api_key("KEY-new", "2026-05-28T22:00:00Z")?);
        assert!(!store.revoke_api_key("KEY-missing", "2026-05-28T22:00:00Z")?);
        assert_eq!(store.verify_api_key("sk_live_new_secret")?, None);
        assert_eq!(
            store.list_api_keys()?[0].revoked_at,
            Some("2026-05-28T22:00:00Z".to_owned())
        );

        Ok(())
    }

    #[test]
    fn errors_by_service_returns_phoenix_group_shape_and_summary()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let mut store = migrated_store()?;
        store.commit_error_ingest(error_ingest(
            "ERR-123456789abc",
            "EVT-123456789abc",
            "group-query-a",
            "2026-05-28T20:00:00Z",
        ))?;

        let result = store.errors_by_service("cadence", "24h", ServiceQueryOptions::default())?;

        assert_eq!(result.service, "cadence");
        assert_eq!(result.window, "24h");
        assert_eq!(result.total_errors, 1);
        assert_eq!(
            result.summary,
            "1 errors in cadence in the last 24h. 1 unique classes. Most frequent: DBConnection.ConnectionError (1 occurrences)."
        );
        assert_eq!(result.cursor, None);
        assert_eq!(result.groups.len(), 1);
        assert_eq!(result.groups[0].group_hash, "group-query-a");
        assert_eq!(result.groups[0].total_count, 1);
        assert_eq!(result.groups[0].classification.category, "infrastructure");

        Ok(())
    }

    #[test]
    fn errors_by_service_cursor_follows_count_then_hash_order()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let store = migrated_store()?;
        let now = "2026-05-28T20:00:00Z";

        for rank in 1..=51 {
            let inverse_hash = format!("{:03}", 52 - rank);
            let group_hash = format!("group-{inverse_hash}");
            store.connection.execute(
                "INSERT INTO error_groups (
                    group_hash, service, error_class, severity, first_seen_at, last_seen_at,
                    last_error_id, total_count, status
                 ) VALUES (?1, 'svc-page', ?2, 'error', ?3, ?3, ?4, ?5, 'active')",
                params![
                    group_hash,
                    format!("RuntimeError{rank}"),
                    now,
                    format!("ERR-page-{rank}"),
                    200 - rank,
                ],
            )?;
        }

        let first_page =
            store.errors_by_service("svc-page", "24h", ServiceQueryOptions::default())?;
        assert_eq!(first_page.groups.len(), 50);
        assert!(first_page.cursor.is_some());

        let second_page = store.errors_by_service(
            "svc-page",
            "24h",
            ServiceQueryOptions {
                cursor: first_page.cursor,
                ..ServiceQueryOptions::default()
            },
        )?;

        assert_eq!(
            second_page
                .groups
                .iter()
                .map(|group| group.group_hash.as_str())
                .collect::<Vec<_>>(),
            vec!["group-001"]
        );
        assert_eq!(second_page.cursor, None);

        Ok(())
    }

    #[test]
    fn errors_by_service_cursor_is_a_keyset_anchor_not_a_snapshot()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let mut store = migrated_store()?;
        let now = "2026-05-28T20:00:00Z";

        for rank in 1..=51 {
            let group_hash = format!("group-{rank:03}");
            store.connection.execute(
                "INSERT INTO error_groups (
                    group_hash, service, error_class, severity, first_seen_at, last_seen_at,
                    last_error_id, total_count, status
                 ) VALUES (?1, 'cadence', ?2, 'error', ?3, ?3, ?4, 49, 'active')",
                params![
                    group_hash,
                    format!("RuntimeError{rank}"),
                    now,
                    format!("ERR-page-{rank}"),
                ],
            )?;
        }

        let first_page =
            store.errors_by_service("cadence", "24h", ServiceQueryOptions::default())?;
        assert_eq!(first_page.groups.len(), 50);
        assert_eq!(first_page.groups[0].group_hash, "group-001");
        assert_eq!(first_page.groups[49].group_hash, "group-050");

        store.commit_error_ingest(error_ingest(
            "ERR-123456789abc",
            "EVT-123456789abc",
            "group-051",
            "2026-05-28T20:01:00Z",
        ))?;

        let second_page = store.errors_by_service(
            "cadence",
            "24h",
            ServiceQueryOptions {
                cursor: first_page.cursor,
                ..ServiceQueryOptions::default()
            },
        )?;

        assert_eq!(second_page.groups, Vec::new());
        let fresh_first_page =
            store.errors_by_service("cadence", "24h", ServiceQueryOptions::default())?;
        assert_eq!(fresh_first_page.groups[0].group_hash, "group-051");
        assert_eq!(fresh_first_page.groups[0].total_count, 50);

        Ok(())
    }

    #[test]
    fn errors_by_class_counts_all_classes_beyond_visible_limit()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let store = migrated_store()?;
        let now = "2026-05-28T20:00:00Z";

        for class_number in 1..=52 {
            store.connection.execute(
                "INSERT INTO error_groups (
                    group_hash, service, error_class, severity, first_seen_at, last_seen_at,
                    last_error_id, total_count, status
                 ) VALUES (?1, ?2, ?3, 'error', ?4, ?4, ?5, 3, 'active')",
                params![
                    format!("group-class-{class_number:03}"),
                    format!("svc-{}", class_number % 3),
                    format!("Err{class_number:03}"),
                    now,
                    format!("ERR-class-{class_number}"),
                ],
            )?;
        }

        let result = store.errors_by_class("24h")?;

        assert_eq!(result.window, "24h");
        assert_eq!(result.groups.len(), 50);
        assert_eq!(result.total_errors, 156);
        assert_eq!(result.total_error_classes, 52);
        assert!(result.truncated);
        assert_eq!(result.groups[0].total_count, 3);
        assert_eq!(result.groups[0].service_count, 1);
        assert_eq!(
            result.summary,
            "156 errors across 52 error classes in the last 24h. Most frequent: Err001 (3 occurrences). Response truncated to top 50 classes."
        );

        Ok(())
    }

    #[test]
    fn health_targets_batch_recent_checks_and_default_missing_state()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let mut store = migrated_store()?;
        store.insert_target(TargetInsert {
            id: "TGT-api".to_owned(),
            url: "https://api.example.test/health".to_owned(),
            name: "API".to_owned(),
            service: "api".to_owned(),
            method: "GET".to_owned(),
            headers: None,
            interval_ms: 60_000,
            timeout_ms: 10_000,
            expected_status: "200".to_owned(),
            body_contains: None,
            degraded_after: 2,
            down_after: 3,
            up_after: 1,
            active: true,
            created_at: "2026-05-28T20:00:00Z".to_owned(),
        })?;
        store.insert_target(TargetInsert {
            id: "TGT-worker".to_owned(),
            url: "https://worker.example.test/health".to_owned(),
            name: "Worker".to_owned(),
            service: "".to_owned(),
            method: "GET".to_owned(),
            headers: None,
            interval_ms: 60_000,
            timeout_ms: 10_000,
            expected_status: "200".to_owned(),
            body_contains: None,
            degraded_after: 2,
            down_after: 3,
            up_after: 1,
            active: true,
            created_at: "2026-05-28T20:00:00Z".to_owned(),
        })?;
        store.connection.execute(
            "INSERT INTO target_state (
                target_id, state, consecutive_failures, last_checked_at, last_success_at
             ) VALUES ('TGT-api', 'down', 3, '2026-05-28T20:05:00Z', '2026-05-28T19:55:00Z')",
            [],
        )?;
        for minute in 0..6 {
            store.connection.execute(
                "INSERT INTO target_checks (
                    target_id, checked_at, status_code, latency_ms, result, tls_expires_at
                 ) VALUES ('TGT-api', ?1, 500, ?2, 'error', ?3)",
                params![
                    format!("2026-05-28T20:0{minute}:00Z"),
                    100 + minute,
                    if minute == 5 {
                        Some("2026-09-01T00:00:00Z")
                    } else {
                        None
                    },
                ],
            )?;
        }

        let targets = store.health_targets()?;

        assert_eq!(targets.len(), 2);
        assert_eq!(targets[0].id, "TGT-api");
        assert_eq!(targets[0].state, "down");
        assert_eq!(targets[0].consecutive_failures, 3);
        assert_eq!(targets[0].latency_ms, Some(105));
        assert_eq!(
            targets[0].tls_expires_at.as_deref(),
            Some("2026-09-01T00:00:00Z")
        );
        assert_eq!(targets[0].recent_checks.len(), 5);
        assert_eq!(
            targets[0].recent_checks[0].checked_at,
            "2026-05-28T20:05:00Z"
        );
        assert_eq!(targets[1].id, "TGT-worker");
        assert_eq!(targets[1].service, "Worker");
        assert_eq!(targets[1].state, "unknown");
        assert_eq!(targets[1].recent_checks, Vec::<HealthCheckSummary>::new());

        Ok(())
    }

    #[test]
    fn target_checks_filters_window_orders_newest_first_and_caps_rows()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let mut store = migrated_store()?;
        store.insert_target(TargetInsert {
            id: "TGT-api".to_owned(),
            url: "https://api.example.test/health".to_owned(),
            name: "API".to_owned(),
            service: "api".to_owned(),
            method: "GET".to_owned(),
            headers: None,
            interval_ms: 60_000,
            timeout_ms: 10_000,
            expected_status: "200".to_owned(),
            body_contains: None,
            degraded_after: 2,
            down_after: 3,
            up_after: 1,
            active: true,
            created_at: "2026-05-28T20:00:00Z".to_owned(),
        })?;
        let now = OffsetDateTime::parse("2026-05-28T21:00:00Z", &Rfc3339)?;
        let old = (now - time::Duration::hours(25)).format(&Rfc3339)?;
        store.connection.execute(
            "INSERT INTO target_checks (
                target_id, checked_at, status_code, latency_ms, result, tls_expires_at, error_detail
             ) VALUES ('TGT-api', ?1, 500, 999, 'error', NULL, 'too old')",
            params![old],
        )?;
        for index in 0..501 {
            let checked_at = (now - time::Duration::seconds(index)).format(&Rfc3339)?;
            store.connection.execute(
                "INSERT INTO target_checks (
                    target_id, checked_at, status_code, latency_ms, result, tls_expires_at, error_detail
                 ) VALUES ('TGT-api', ?1, 200, ?2, 'ok', ?3, NULL)",
                params![
                    checked_at,
                    index,
                    if index == 0 {
                        Some("2026-09-01T00:00:00Z")
                    } else {
                        None
                    },
                ],
            )?;
        }

        let checks = health::target_checks_at(&store.connection, "TGT-api", "24h", now)?;

        assert_eq!(checks.len(), 500);
        assert_eq!(checks[0].latency_ms, Some(0));
        assert_eq!(
            checks[0].tls_expires_at.as_deref(),
            Some("2026-09-01T00:00:00Z")
        );
        assert_eq!(checks[499].latency_ms, Some(499));
        assert!(
            !checks
                .iter()
                .any(|check| check.error_detail.as_deref() == Some("too old"))
        );
        assert!(matches!(
            health::target_checks_at(&store.connection, "TGT-api", "99h", now),
            Err(QueryError::InvalidWindow)
        ));

        Ok(())
    }

    #[test]
    fn retention_prune_deletes_old_rows_in_batches_and_keeps_recent_rows()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let mut store = migrated_store()?;
        store.insert_target(TargetInsert {
            id: "TGT-retention".to_owned(),
            url: "https://retention.example.test/health".to_owned(),
            name: "retention".to_owned(),
            service: "retention".to_owned(),
            method: "GET".to_owned(),
            headers: None,
            interval_ms: 60_000,
            timeout_ms: 10_000,
            expected_status: "200".to_owned(),
            body_contains: None,
            degraded_after: 2,
            down_after: 3,
            up_after: 1,
            active: true,
            created_at: "2026-05-28T20:00:00Z".to_owned(),
        })?;

        for index in 0..1005 {
            store.connection.execute(
                "INSERT INTO errors (
                    id, service, error_class, message, group_hash, created_at
                 ) VALUES (?1, 'retention', 'RuntimeError', 'old', ?2, '2026-04-01T00:00:00Z')",
                params![format!("ERR-old-{index}"), format!("grp-old-{index}")],
            )?;
        }
        store.connection.execute(
            "INSERT INTO errors (
                id, service, error_class, message, group_hash, created_at
             ) VALUES ('ERR-recent', 'retention', 'RuntimeError', 'recent', 'grp-recent', '2026-05-28T00:00:00Z')",
            [],
        )?;

        store.connection.execute(
            "INSERT INTO service_events (
                id, service, event, entity_type, entity_ref, severity, summary, payload, created_at
             ) VALUES (
                'EVT-old', 'retention', 'error.new_class', 'error_group', 'grp-old',
                'error', 'old event', '{\"event\":\"error.new_class\"}', '2026-04-01T00:00:00Z'
             )",
            [],
        )?;
        store.connection.execute(
            "INSERT INTO service_events (
                id, service, event, entity_type, entity_ref, severity, summary, payload, created_at
             ) VALUES (
                'EVT-recent', 'retention', 'error.new_class', 'error_group', 'grp-recent',
                'error', 'recent event', '{\"event\":\"error.new_class\"}', '2026-05-28T00:00:00Z'
             )",
            [],
        )?;

        store.connection.execute(
            "INSERT INTO target_checks (target_id, checked_at, result)
             VALUES ('TGT-retention', '2026-05-01T00:00:00Z', 'success')",
            [],
        )?;
        store.connection.execute(
            "INSERT INTO target_checks (target_id, checked_at, result)
             VALUES ('TGT-retention', '2026-05-28T00:00:00Z', 'success')",
            [],
        )?;

        let report = store.prune_retention(RetentionPrune {
            error_cutoff: "2026-05-01T00:00:00Z".to_owned(),
            check_cutoff: "2026-05-22T00:00:00Z".to_owned(),
        })?;

        assert_eq!(
            report,
            RetentionPruneReport {
                errors_deleted: 1005,
                service_events_deleted: 1,
                target_checks_deleted: 1,
            }
        );
        assert_eq!(row_count(&store.connection, "errors")?, 1);
        assert_eq!(row_count(&store.connection, "service_events")?, 1);
        assert_eq!(row_count(&store.connection, "target_checks")?, 1);
        assert_eq!(
            store
                .connection
                .query_row("SELECT id FROM errors", [], |row| row.get::<_, String>(0))?,
            "ERR-recent"
        );

        Ok(())
    }

    #[test]
    fn retention_prune_batch_deletes_only_one_bounded_batch()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let mut store = migrated_store()?;

        for index in 0..1005 {
            store.connection.execute(
                "INSERT INTO errors (
                    id, service, error_class, message, group_hash, created_at
                 ) VALUES (?1, 'retention', 'RuntimeError', 'old', ?2, '2026-04-01T00:00:00Z')",
                params![format!("ERR-batch-{index}"), format!("grp-batch-{index}")],
            )?;
        }

        let first = store.prune_retention_batch(RetentionPruneBatch {
            table: RetentionPruneTable::Errors,
            cutoff: "2026-05-01T00:00:00Z".to_owned(),
        })?;
        assert_eq!(
            first,
            RetentionPruneBatchReport {
                deleted: 1000,
                complete: false,
            }
        );
        assert_eq!(row_count(&store.connection, "errors")?, 5);

        let second = store.prune_retention_batch(RetentionPruneBatch {
            table: RetentionPruneTable::Errors,
            cutoff: "2026-05-01T00:00:00Z".to_owned(),
        })?;
        assert_eq!(
            second,
            RetentionPruneBatchReport {
                deleted: 5,
                complete: true,
            }
        );
        assert_eq!(row_count(&store.connection, "errors")?, 0);

        Ok(())
    }

    #[test]
    fn error_summary_counts_active_groups_inside_window()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let store = migrated_store()?;
        for (hash, service, last_seen_at, count, status) in [
            ("group-a", "api", "2026-05-28T20:00:00Z", 7, "active"),
            ("group-b", "api", "2026-05-28T19:00:00Z", 5, "active"),
            ("group-c", "worker", "2026-05-28T20:30:00Z", 11, "active"),
            ("group-old", "api", "2026-05-27T20:00:00Z", 99, "active"),
            (
                "group-resolved",
                "api",
                "2026-05-28T20:00:00Z",
                99,
                "resolved",
            ),
        ] {
            store.connection.execute(
                "INSERT INTO error_groups (
                    group_hash, service, error_class, severity, first_seen_at, last_seen_at,
                    last_error_id, total_count, status
                 ) VALUES (?1, ?2, 'RuntimeError', 'error', ?3, ?3, ?4, ?5, ?6)",
                params![
                    hash,
                    service,
                    last_seen_at,
                    format!("ERR-{hash}"),
                    count,
                    status
                ],
            )?;
        }
        let now = OffsetDateTime::parse("2026-05-28T21:00:00Z", &Rfc3339)?;

        let summary = query::error_summary_at(&store.connection, "6h", now)?;

        assert_eq!(summary.len(), 2);
        assert_eq!(summary[0].service, "api");
        assert_eq!(summary[0].total_count, 12);
        assert_eq!(summary[0].unique_classes, 2);
        assert_eq!(summary[1].service, "worker");
        assert_eq!(summary[1].total_count, 11);
        assert_eq!(summary[1].unique_classes, 1);

        Ok(())
    }

    #[test]
    fn report_read_models_match_phoenix_filters_ordering_and_search()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let mut store = migrated_store()?;
        store.insert_target(TargetInsert {
            id: "TGT-api".to_owned(),
            url: "https://example.com/api/health".to_owned(),
            name: "api".to_owned(),
            service: "api".to_owned(),
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
            created_at: "2026-05-28T20:00:00Z".to_owned(),
        })?;
        store.connection.execute(
            "INSERT INTO target_state (
                target_id, state, consecutive_failures, last_checked_at, last_transition_at
             ) VALUES ('TGT-api', 'degraded', 2, '2026-05-28T20:55:00Z', '2026-05-28T20:55:00Z')",
            [],
        )?;
        for (hash, service, class, count, status) in [
            ("group-a", "api", "TimeoutError", 7, "active"),
            ("group-b", "worker", "RuntimeError", 5, "active"),
            ("group-resolved", "api", "ResolvedError", 99, "resolved"),
        ] {
            store.connection.execute(
                "INSERT INTO errors (
                    id, service, error_class, message, stack_trace, group_hash, created_at,
                    classification_category, classification_persistence, classification_component
                 ) VALUES (?1, ?2, ?3, ?4, 'stack', ?5, '2026-05-28T20:50:00Z',
                    'runtime', 'transient', 'application')",
                params![
                    format!("ERR-{hash}"),
                    service,
                    class,
                    format!("timeout while reporting {service}"),
                    hash
                ],
            )?;
            store.connection.execute(
                "INSERT INTO error_groups (
                    group_hash, service, error_class, severity, first_seen_at, last_seen_at,
                    last_error_id, total_count, status
                 ) VALUES (?1, ?2, ?3, 'error', '2026-05-28T20:00:00Z',
                    '2026-05-28T20:50:00Z', ?4, ?5, ?6)",
                params![hash, service, class, format!("ERR-{hash}"), count, status],
            )?;
        }
        let now = OffsetDateTime::parse("2026-05-28T21:00:00Z", &Rfc3339)?;

        let groups = store.report_error_groups_at("1h", now)?;
        assert_eq!(
            groups
                .iter()
                .map(|group| group.group_hash.as_str())
                .collect::<Vec<_>>(),
            vec!["group-a", "group-b"]
        );
        assert_eq!(groups[0].classification.category, "runtime");

        let transitions = query::recent_transitions_at(&store.connection, "1h", now)?;
        assert_eq!(transitions.len(), 1);
        assert_eq!(transitions[0].entity_type, "target");
        assert_eq!(transitions[0].entity_ref, "TGT-api");

        let search = query::search_errors_at(&store.connection, "timeout", "1h", now)?;
        assert_eq!(search.len(), 3);
        assert!(search.iter().any(|result| result.service == "api"));
        assert!(matches!(
            query::report_error_groups_at(&store.connection, "99h", now),
            Err(QueryError::InvalidWindow)
        ));

        Ok(())
    }

    #[test]
    fn timeline_filters_paginates_and_rejects_invalid_inputs()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let store = migrated_store()?;
        for (id, service, event, created_at) in [
            ("EVT-a", "alpha", "incident.opened", "2026-05-28T20:59:50Z"),
            ("EVT-b", "alpha", "error.new_class", "2026-05-28T20:59:50Z"),
            ("EVT-c", "beta", "error.new_class", "2026-05-28T20:59:40Z"),
            (
                "EVT-old",
                "alpha",
                "error.new_class",
                "2026-05-27T19:00:00Z",
            ),
        ] {
            store.connection.execute(
                "INSERT INTO service_events (
                    id, service, event, entity_type, entity_ref, severity, summary, payload, created_at
                 ) VALUES (?1, ?2, ?3, 'error_group', 'group-a', 'error', 'summary', ?4, ?5)",
                params![id, service, event, json!({"event": event}).to_string(), created_at],
            )?;
        }
        let now = OffsetDateTime::parse("2026-05-28T21:00:00Z", &Rfc3339)?;

        let first = query::timeline_at(
            &store.connection,
            "24h",
            TimelineQueryOptions {
                service: Some("alpha".to_owned()),
                limit: Some("1".to_owned()),
                cursor: None,
                event_type: None,
            },
            now,
        )?;

        assert_eq!(first.returned_count, 1);
        assert_eq!(first.service.as_deref(), Some("alpha"));
        assert_eq!(first.events[0].id, "EVT-b");
        assert_eq!(first.events[0].payload["event"], "error.new_class");
        assert!(first.cursor.is_some());

        let second = query::timeline_at(
            &store.connection,
            "24h",
            TimelineQueryOptions {
                service: Some("alpha".to_owned()),
                limit: Some("1".to_owned()),
                cursor: first.cursor,
                event_type: None,
            },
            now,
        )?;

        assert_eq!(second.events[0].id, "EVT-a");
        assert!(second.cursor.is_none());

        let event_filtered = query::timeline_at(
            &store.connection,
            "24h",
            TimelineQueryOptions {
                service: None,
                limit: None,
                cursor: None,
                event_type: Some("incident.opened, error.new_class".to_owned()),
            },
            now,
        )?;
        assert_eq!(event_filtered.returned_count, 3);
        assert!(
            !event_filtered
                .events
                .iter()
                .any(|event| event.id == "EVT-old")
        );

        assert!(matches!(
            query::timeline_at(
                &store.connection,
                "99h",
                TimelineQueryOptions::default(),
                now
            ),
            Err(TimelineQueryError::InvalidWindow)
        ));
        assert!(matches!(
            query::timeline_at(
                &store.connection,
                "24h",
                TimelineQueryOptions {
                    limit: Some("201".to_owned()),
                    ..TimelineQueryOptions::default()
                },
                now
            ),
            Err(TimelineQueryError::InvalidLimit)
        ));
        assert!(matches!(
            query::timeline_at(
                &store.connection,
                "24h",
                TimelineQueryOptions {
                    cursor: Some("bogus".to_owned()),
                    ..TimelineQueryOptions::default()
                },
                now
            ),
            Err(TimelineQueryError::InvalidCursor)
        ));
        assert!(matches!(
            query::timeline_at(
                &store.connection,
                "24h",
                TimelineQueryOptions {
                    event_type: Some("canary.ping".to_owned()),
                    ..TimelineQueryOptions::default()
                },
                now
            ),
            Err(TimelineQueryError::InvalidEventType(invalid)) if invalid == vec!["canary.ping"]
        ));

        Ok(())
    }

    #[test]
    fn active_incidents_filters_inactive_signals_and_annotation_actions()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let store = migrated_store()?;
        let now =
            OffsetDateTime::now_utc().format(&time::format_description::well_known::Rfc3339)?;

        store.connection.execute(
            "INSERT INTO target_state (target_id, state, consecutive_failures)
             VALUES ('TGT-api', 'down', 2)",
            [],
        )?;
        store.connection.execute(
            "INSERT INTO target_state (target_id, state, consecutive_failures)
             VALUES ('TGT-web', 'up', 0)",
            [],
        )?;
        insert_incident(&store, "INC-api", "api", "2026-05-28T20:00:00Z")?;
        insert_incident(&store, "INC-web", "web", "2026-05-28T19:00:00Z")?;
        insert_incident_signal(
            &store,
            "INC-api",
            "health_transition",
            "TGT-api",
            &now,
            None,
        )?;
        insert_incident_signal(
            &store,
            "INC-web",
            "health_transition",
            "TGT-web",
            &now,
            None,
        )?;
        store.connection.execute(
            "INSERT INTO annotations (
                id, incident_id, agent, action, created_at, subject_type, subject_id
             ) VALUES ('ANN-api', 'INC-api', 'agent', 'acknowledged', ?1, 'incident', 'INC-api')",
            [now.as_str()],
        )?;

        let all = store.active_incidents(IncidentListOptions::default())?;
        assert_eq!(all.incidents.len(), 1);
        assert_eq!(all.incidents[0].id, "INC-api");
        assert_eq!(all.incidents[0].state, "investigating");
        assert_eq!(all.incidents[0].severity, "medium");
        assert_eq!(all.incidents[0].signal_count, 1);
        assert_eq!(
            all.summary,
            "1 open incident across 1 service. Newest: api at 2026-05-28T20:00:00Z."
        );

        let with = store.active_incidents(IncidentListOptions {
            with_annotation: Some("acknowledged".to_owned()),
            without_annotation: None,
        })?;
        assert_eq!(with.incidents.len(), 1);

        let without = store.active_incidents(IncidentListOptions {
            with_annotation: None,
            without_annotation: Some("acknowledged".to_owned()),
        })?;
        assert_eq!(without.incidents, Vec::new());
        assert_eq!(without.summary, "No active incidents.");

        Ok(())
    }

    #[test]
    fn active_incidents_uses_typed_health_state_activity_contract()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let mut store = migrated_store()?;
        let now =
            OffsetDateTime::now_utc().format(&time::format_description::well_known::Rfc3339)?;

        for (incident_id, reference, state, table, id_column) in [
            (
                "INC-unknown",
                "TGT-unknown",
                HealthState::Unknown.as_str(),
                "target_state",
                "target_id",
            ),
            (
                "INC-degraded",
                "TGT-degraded",
                HealthState::Degraded.as_str(),
                "target_state",
                "target_id",
            ),
            (
                "INC-down",
                "TGT-down",
                HealthState::Down.as_str(),
                "target_state",
                "target_id",
            ),
            (
                "INC-paused",
                "TGT-paused",
                HealthState::Paused.as_str(),
                "target_state",
                "target_id",
            ),
            (
                "INC-flapping-monitor",
                "MON-flapping",
                HealthState::Flapping.as_str(),
                "monitor_state",
                "monitor_id",
            ),
            (
                "INC-up",
                "TGT-up",
                HealthState::Up.as_str(),
                "target_state",
                "target_id",
            ),
        ] {
            if reference.starts_with("TGT-") {
                store.insert_target(TargetInsert {
                    id: reference.to_owned(),
                    url: format!("https://{}.example.com", reference.to_lowercase()),
                    name: reference.to_owned(),
                    service: reference.to_owned(),
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
                })?;
            } else {
                store.insert_monitor(MonitorInsert {
                    id: reference.to_owned(),
                    name: reference.to_owned(),
                    service: reference.to_owned(),
                    mode: "ttl".to_owned(),
                    expected_every_ms: 60_000,
                    grace_ms: 5_000,
                    created_at: "2026-05-28T19:00:00Z".to_owned(),
                })?;
            }

            store.connection.execute(
                &format!("INSERT INTO {table} ({id_column}, state) VALUES (?1, ?2)"),
                params![reference, state],
            )?;
            insert_incident(&store, incident_id, reference, "2026-05-28T20:00:00Z")?;
            insert_incident_signal(
                &store,
                incident_id,
                "health_transition",
                reference,
                &now,
                None,
            )?;
        }

        insert_incident(&store, "INC-missing", "missing", "2026-05-28T20:00:00Z")?;
        insert_incident_signal(
            &store,
            "INC-missing",
            "health_transition",
            "TGT-missing",
            &now,
            None,
        )?;

        let active_ids = store
            .active_incidents(IncidentListOptions::default())?
            .incidents
            .into_iter()
            .map(|incident| incident.id)
            .collect::<BTreeSet<_>>();

        assert_eq!(
            active_ids,
            BTreeSet::from([
                "INC-degraded".to_owned(),
                "INC-down".to_owned(),
                "INC-flapping-monitor".to_owned(),
                "INC-paused".to_owned(),
                "INC-unknown".to_owned(),
            ])
        );

        Ok(())
    }

    #[test]
    fn active_incidents_marks_high_severity_after_three_recent_signals()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let store = migrated_store()?;
        let now =
            OffsetDateTime::now_utc().format(&time::format_description::well_known::Rfc3339)?;

        insert_incident(&store, "INC-high", "api", "2026-05-28T20:00:00Z")?;
        for index in 1..=3 {
            let group_hash = format!("group-high-{index}");
            store.connection.execute(
                "INSERT INTO error_groups (
                    group_hash, service, error_class, severity, first_seen_at, last_seen_at,
                    last_error_id, total_count, status
                 ) VALUES (?1, 'api', ?2, 'error', ?3, ?3, ?4, 1, 'active')",
                params![
                    group_hash,
                    format!("HighError{index}"),
                    now,
                    format!("ERR-high-{index}"),
                ],
            )?;
            insert_incident_signal(&store, "INC-high", "error_group", &group_hash, &now, None)?;
        }

        let result = store.active_incidents(IncidentListOptions::default())?;

        assert_eq!(result.incidents.len(), 1);
        assert_eq!(result.incidents[0].severity, "high");
        assert_eq!(result.incidents[0].signal_count, 3);
        assert_eq!(
            result.summary,
            "1 open incident across 1 service. 1 high-severity incident. Newest: api at 2026-05-28T20:00:00Z."
        );

        Ok(())
    }

    #[test]
    fn active_incidents_keeps_persistent_health_signals_severity_relevant()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let mut store = migrated_store()?;
        let attached_at = "2026-05-28T20:00:00Z";
        let as_of = OffsetDateTime::parse("2026-05-28T20:10:00Z", &Rfc3339)?;

        insert_incident(&store, "INC-health-high", "api", "2026-05-28T20:00:00Z")?;
        for index in 1..=3 {
            let target_id = format!("TGT-health-{index}");
            store.insert_target(TargetInsert {
                id: target_id.clone(),
                url: format!("https://health-{index}.example.com"),
                name: format!("Health {index}"),
                service: "api".to_owned(),
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
            })?;
            store.connection.execute(
                "INSERT INTO target_state (target_id, state)
                 VALUES (?1, 'down')",
                [target_id.as_str()],
            )?;
            insert_incident_signal(
                &store,
                "INC-health-high",
                "health_transition",
                &target_id,
                attached_at,
                None,
            )?;
        }

        let result = store.active_incidents_at(IncidentListOptions::default(), as_of)?;

        assert_eq!(result.incidents.len(), 1);
        assert_eq!(result.incidents[0].severity, "high");
        assert_eq!(result.incidents[0].signal_count, 3);
        assert_eq!(
            result.summary,
            "1 open incident across 1 service. 1 high-severity incident. Newest: api at 2026-05-28T20:00:00Z."
        );

        Ok(())
    }

    #[test]
    fn incident_detail_returns_bounded_context_and_action_brief()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let store = migrated_store()?;
        insert_incident(&store, "INC-detail", "api", "2026-05-28T20:00:00Z")?;
        store.connection.execute(
            "INSERT INTO targets (id, url, name, created_at, service)
             VALUES ('TGT-api', 'https://api.example.com', 'API', '2026-05-28T19:00:00Z', 'api')",
            [],
        )?;
        store.connection.execute(
            "INSERT INTO target_state (target_id, state, consecutive_failures)
             VALUES ('TGT-api', 'down', 4)",
            [],
        )?;
        store.connection.execute(
            "INSERT INTO monitors (id, name, service, mode, expected_every_ms, created_at)
             VALUES ('MON-api-cron', 'API cron', 'api', 'ttl', 60000, '2026-05-28T19:00:00Z')",
            [],
        )?;
        store.connection.execute(
            "INSERT INTO monitor_state (monitor_id, state)
             VALUES ('MON-api-cron', 'down')",
            [],
        )?;
        store.connection.execute(
            "INSERT INTO errors (
                id, service, error_class, message, group_hash, created_at,
                classification_category, classification_persistence, classification_component
             ) VALUES (
                'ERR-detail', 'api', 'DetailError', 'boom', 'group-detail',
                '2026-05-28T20:00:00Z', 'application', 'persistent', 'runtime'
             )",
            [],
        )?;
        store.connection.execute(
            "INSERT INTO error_groups (
                group_hash, service, error_class, severity, first_seen_at, last_seen_at,
                total_count, last_error_id, status
             ) VALUES (
                'group-detail', 'api', 'DetailError', 'error',
                '2026-05-28T19:55:00Z', '2026-05-28T20:00:00Z', 4,
                'ERR-detail', 'active'
             )",
            [],
        )?;
        insert_incident_signal(
            &store,
            "INC-detail",
            "health_transition",
            "TGT-api",
            "2026-05-28T20:02:00Z",
            None,
        )?;
        insert_incident_signal(
            &store,
            "INC-detail",
            "error_group",
            "group-detail",
            "2026-05-28T20:01:00Z",
            None,
        )?;
        insert_incident_signal(
            &store,
            "INC-detail",
            "health_transition",
            "MON-api-cron",
            "2026-05-28T20:00:30Z",
            Some("2026-05-28T20:03:00Z"),
        )?;
        store.connection.execute(
            "INSERT INTO annotations (
                id, incident_id, group_hash, agent, action, metadata, created_at, subject_type, subject_id
             ) VALUES (
                'ANN-incident', 'INC-detail', NULL, 'codex', 'acknowledged',
                '{\"deployment\":\"https://example.com/deploy\"}', '2026-05-28T20:04:00Z',
                'incident', 'INC-detail'
             )",
            [],
        )?;
        for id in ["ANN-g1", "ANN-g2", "ANN-g3"] {
            store.connection.execute(
                "INSERT INTO annotations (
                    id, agent, action, created_at, subject_type, subject_id
                 ) VALUES (?1, 'agent', 'triaged', '2026-05-28T20:04:00Z', 'error_group', 'group-detail')",
                [id],
            )?;
        }
        store.connection.execute(
            "INSERT INTO annotations (
                id, agent, action, created_at, subject_type, subject_id
             ) VALUES (
                'ANN-target', 'agent', 'triaged', '2026-05-28T20:04:00Z', 'target', 'TGT-api'
             )",
            [],
        )?;
        store.connection.execute(
            "INSERT INTO service_events (
                id, service, event, entity_type, entity_ref, severity, summary, payload, created_at
             ) VALUES (
                'EVT-incident', 'api', 'incident.opened', 'incident', 'INC-detail',
                'warning', 'api incident opened', '{}', '2026-05-28T20:00:00Z'
             )",
            [],
        )?;

        let detail = store
            .incident_detail("INC-detail")?
            .ok_or(StoreError::Sqlite(rusqlite::Error::QueryReturnedNoRows))?;

        assert_eq!(detail.incident.id, "INC-detail");
        assert_eq!(detail.incident.state, "investigating");
        assert_eq!(detail.incident.signal_count, 3);
        assert_eq!(detail.signals.len(), 3);
        assert!(!detail.signals_truncated);
        assert_eq!(detail.annotations.len(), 1);
        assert!(!detail.annotations_truncated);
        assert_eq!(detail.annotations[0].action, "acknowledged");
        assert_eq!(
            detail.annotations[0]
                .metadata
                .as_ref()
                .and_then(|value| value.get("deployment"))
                .and_then(Value::as_str),
            Some("https://example.com/deploy")
        );
        assert_eq!(detail.recent_timeline_events[0].event, "incident.opened");

        let target = detail
            .signals
            .iter()
            .find(|signal| signal.target_id.as_deref() == Some("TGT-api"))
            .ok_or("missing target signal")?;
        assert_eq!(target.target_name.as_deref(), Some("API"));
        assert_eq!(target.current_state.as_deref(), Some("down"));
        assert_eq!(target.consecutive_failures, Some(4));
        assert_eq!(target.annotation_count, 1);

        let group = detail
            .signals
            .iter()
            .find(|signal| signal.group_hash.as_deref() == Some("group-detail"))
            .ok_or("missing error group signal")?;
        assert_eq!(group.error_class.as_deref(), Some("DetailError"));
        assert_eq!(group.total_count, Some(4));
        assert_eq!(
            group
                .classification
                .as_ref()
                .map(|classification| classification.category.as_str()),
            Some("application")
        );
        assert_eq!(group.annotation_count, 3);

        let monitor = detail
            .signals
            .iter()
            .find(|signal| signal.monitor_id.as_deref() == Some("MON-api-cron"))
            .ok_or("missing monitor signal")?;
        assert_eq!(monitor.monitor_name.as_deref(), Some("API cron"));
        assert_eq!(monitor.annotation_count, 0);

        assert_eq!(detail.action_brief.recommendation.action, "watch");
        assert_eq!(detail.action_brief.signal_counts.active, 2);
        assert_eq!(detail.action_brief.signal_counts.resolved, 1);
        assert_eq!(
            detail
                .action_brief
                .latest_annotation
                .as_ref()
                .map(|annotation| annotation.action.as_str()),
            Some("acknowledged")
        );

        Ok(())
    }

    #[test]
    fn incident_detail_caps_signals_and_annotations_with_conservative_brief()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let store = migrated_store()?;
        insert_incident(&store, "INC-cap", "api", "2026-05-28T20:00:00Z")?;
        for index in 1..=30 {
            insert_incident_signal(
                &store,
                "INC-cap",
                "error_group",
                &format!("group-{index:03}"),
                "2026-05-28T20:00:00Z",
                Some("2026-05-28T20:05:00Z"),
            )?;
        }
        for index in 1..=25 {
            store.connection.execute(
                "INSERT INTO annotations (
                    id, incident_id, agent, action, created_at, subject_type, subject_id
                 ) VALUES (?1, 'INC-cap', 'agent', ?2, ?3, 'incident', 'INC-cap')",
                params![
                    format!("ANN-cap-{index:03}"),
                    format!("note-{index}"),
                    format!("2026-05-28T20:{index:02}:00Z"),
                ],
            )?;
        }

        let detail = store
            .incident_detail("INC-cap")?
            .ok_or(StoreError::Sqlite(rusqlite::Error::QueryReturnedNoRows))?;

        assert_eq!(detail.incident.signal_count, 30);
        assert_eq!(detail.signals.len(), 25);
        assert!(detail.signals_truncated);
        assert_eq!(detail.annotations.len(), 20);
        assert!(detail.annotations_truncated);
        assert_eq!(detail.annotations[0].action, "note-25");
        assert!(!detail.annotations.iter().any(|ann| ann.action == "note-1"));
        assert_eq!(
            detail.action_brief.recommendation.action,
            "inspect-truncated-signals"
        );
        assert_eq!(detail.action_brief.signal_counts.visible, 25);
        assert_eq!(detail.action_brief.signal_counts.total, 30);

        Ok(())
    }

    #[test]
    fn incident_detail_returns_none_for_unknown_incident()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let store = migrated_store()?;

        assert!(store.incident_detail("INC-missing")?.is_none());

        Ok(())
    }

    #[test]
    fn error_detail_returns_group_context_and_incident_ids()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let mut store = migrated_store()?;
        store.commit_error_ingest(error_ingest(
            "ERR-123456789abc",
            "EVT-123456789abc",
            "group-detail",
            "2026-05-28T20:00:00Z",
        ))?;
        store.connection.execute(
            "INSERT INTO incidents (id, service, state, severity, opened_at)
             VALUES ('INC-123456789abc', 'cadence', 'investigating', 'medium', '2026-05-28T20:00:00Z')",
            [],
        )?;
        store.connection.execute(
            "INSERT INTO incident_signals (incident_id, signal_type, signal_ref, attached_at)
             VALUES ('INC-123456789abc', 'error_group', 'group-detail', '2026-05-28T20:00:00Z')",
            [],
        )?;

        let detail = store
            .error_detail("ERR-123456789abc")?
            .ok_or(StoreError::Sqlite(rusqlite::Error::QueryReturnedNoRows))?;

        assert_eq!(detail.service, "cadence");
        assert_eq!(detail.group_hash, "group-detail");
        assert_eq!(detail.incident_ids, vec!["INC-123456789abc"]);
        assert_eq!(
            detail.summary,
            "DBConnection.ConnectionError in cadence. Seen 1 times since 2026-05-28T20:00:00Z. Last occurrence: 2026-05-28T20:00:00Z."
        );
        assert_eq!(
            detail
                .context
                .as_ref()
                .and_then(|value| value.get("tenant"))
                .and_then(Value::as_str),
            Some("alpha")
        );
        assert_eq!(
            detail.group.as_ref().map(|group| group.total_count),
            Some(1)
        );

        Ok(())
    }

    #[test]
    fn monitor_tables_preserve_cascade_and_defaults() -> Result<()> {
        let store = migrated_store()?;
        let monitor_state = columns(&store.connection, "monitor_state")?;

        assert_column(
            &monitor_state,
            "monitor_id",
            ColumnSpec::new("TEXT").primary_key_position(1),
        );
        assert_column(
            &monitor_state,
            "state",
            ColumnSpec::new("TEXT")
                .not_null()
                .default_value("'unknown'"),
        );
        assert_column(
            &monitor_state,
            "sequence",
            ColumnSpec::new("INTEGER").not_null().default_value("0"),
        );
        assert_eq!(
            foreign_keys(&store.connection, "monitor_state")?,
            vec![ForeignKey {
                table: "monitors".to_owned(),
                from: "monitor_id".to_owned(),
                to: "id".to_owned(),
                on_delete: "CASCADE".to_owned(),
            }]
        );

        Ok(())
    }

    #[test]
    fn monitor_check_in_snapshot_finds_monitor_and_ensures_unknown_state() -> Result<()> {
        let mut store = migrated_store()?;
        store.insert_monitor(MonitorInsert {
            id: "MON-worker".to_owned(),
            name: "Worker heartbeat".to_owned(),
            service: "worker".to_owned(),
            mode: "ttl".to_owned(),
            expected_every_ms: 60_000,
            grace_ms: 5_000,
            created_at: "2026-05-28T19:00:00Z".to_owned(),
        })?;

        let snapshot = store
            .monitor_check_in_snapshot_by_name("Worker heartbeat")?
            .ok_or(rusqlite::Error::QueryReturnedNoRows)?;

        assert_eq!(snapshot.id, "MON-worker");
        assert_eq!(snapshot.service, "worker");
        assert_eq!(snapshot.mode, "ttl");
        assert_eq!(snapshot.expected_every_ms, 60_000);
        assert_eq!(snapshot.grace_ms, 5_000);
        assert_eq!(snapshot.state, "unknown");
        assert_eq!(
            store.connection.query_row(
                "SELECT state FROM monitor_state WHERE monitor_id = 'MON-worker'",
                [],
                |row| row.get::<_, String>(0),
            )?,
            "unknown"
        );

        Ok(())
    }

    #[test]
    fn target_probe_snapshot_finds_active_target_and_ensures_unknown_state() -> Result<()> {
        let mut store = migrated_store()?;
        store.insert_target(TargetInsert {
            id: "TGT-api".to_owned(),
            url: "https://api.example.test/health".to_owned(),
            name: "api-web".to_owned(),
            service: "api".to_owned(),
            method: "GET".to_owned(),
            headers: Some(r#"{"x-canary":"yes"}"#.to_owned()),
            interval_ms: 60_000,
            timeout_ms: 7_500,
            expected_status: "200-299".to_owned(),
            body_contains: Some("ok".to_owned()),
            degraded_after: 2,
            down_after: 4,
            up_after: 2,
            active: true,
            created_at: "2026-05-28T19:00:00Z".to_owned(),
        })?;

        let snapshot = store
            .target_probe_snapshot_by_id("TGT-api")?
            .ok_or(rusqlite::Error::QueryReturnedNoRows)?;

        assert_eq!(snapshot.name, "api-web");
        assert_eq!(snapshot.service, "api");
        assert_eq!(snapshot.url, "https://api.example.test/health");
        assert_eq!(snapshot.method, "GET");
        assert_eq!(snapshot.timeout_ms, 7_500);
        assert_eq!(snapshot.expected_status, "200-299");
        assert_eq!(snapshot.body_contains.as_deref(), Some("ok"));
        assert_eq!(snapshot.degraded_after, 2);
        assert_eq!(snapshot.down_after, 4);
        assert_eq!(snapshot.up_after, 2);
        assert_eq!(snapshot.state, "unknown");
        assert_eq!(snapshot.consecutive_failures, 0);
        assert_eq!(snapshot.consecutive_successes, 0);
        assert_eq!(
            store.connection.query_row(
                "SELECT state FROM target_state WHERE target_id = 'TGT-api'",
                [],
                |row| row.get::<_, String>(0),
            )?,
            "unknown"
        );

        Ok(())
    }

    #[test]
    fn active_target_probe_schedules_return_only_active_targets_ordered_by_id() -> Result<()> {
        let mut store = migrated_store()?;
        for (id, active, interval_ms) in [
            ("TGT-b", true, 45_000),
            ("TGT-inactive", false, 60_000),
            ("TGT-a", true, 30_000),
        ] {
            store.insert_target(TargetInsert {
                id: id.to_owned(),
                url: format!("https://{id}.example.test/health"),
                name: id.to_owned(),
                service: id.to_owned(),
                method: "GET".to_owned(),
                headers: None,
                interval_ms,
                timeout_ms: 7_500,
                expected_status: "200".to_owned(),
                body_contains: None,
                degraded_after: 1,
                down_after: 3,
                up_after: 1,
                active,
                created_at: "2026-05-28T19:00:00Z".to_owned(),
            })?;
        }

        let schedules = store.active_target_probe_schedules()?;

        assert_eq!(
            schedules,
            vec![
                ActiveTargetProbeSchedule {
                    target_id: "TGT-a".to_owned(),
                    interval_ms: 30_000,
                },
                ActiveTargetProbeSchedule {
                    target_id: "TGT-b".to_owned(),
                    interval_ms: 45_000,
                },
            ]
        );
        Ok(())
    }

    #[test]
    fn admin_target_list_active_update_and_delete_are_persistent() -> Result<()> {
        let mut store = migrated_store()?;
        store.insert_target(TargetInsert {
            id: "TGT-admin".to_owned(),
            url: "https://admin.example.test/health".to_owned(),
            name: "Admin API".to_owned(),
            service: "".to_owned(),
            method: "HEAD".to_owned(),
            headers: None,
            interval_ms: 15_000,
            timeout_ms: 2_500,
            expected_status: "204".to_owned(),
            body_contains: None,
            degraded_after: 1,
            down_after: 2,
            up_after: 1,
            active: true,
            created_at: "2026-05-28T19:00:00Z".to_owned(),
        })?;
        store.connection.execute(
            "INSERT INTO target_state (target_id, state) VALUES ('TGT-admin', 'up')",
            [],
        )?;

        let targets = store.list_targets()?;
        assert_eq!(targets.len(), 1);
        assert_eq!(targets[0].service, "Admin API");
        assert_eq!(targets[0].method, "HEAD");
        assert!(targets[0].active);

        let interval_update = store
            .update_target_interval("TGT-admin", 30_000)?
            .ok_or(StoreError::Sqlite(rusqlite::Error::QueryReturnedNoRows))?;
        assert_eq!(interval_update.prior_interval_ms, 15_000);
        assert!(interval_update.prior_active);
        assert_eq!(interval_update.target.interval_ms, 30_000);
        assert_eq!(interval_update.target.created_at, "2026-05-28T19:00:00Z");
        assert_eq!(
            store.connection.query_row(
                "SELECT interval_ms FROM targets WHERE id = 'TGT-admin'",
                [],
                |row| row.get::<_, i64>(0),
            )?,
            30_000
        );
        assert!(
            store
                .update_target_interval("TGT-missing", 30_000)?
                .is_none()
        );

        assert!(store.update_target_active("TGT-admin", false)?);
        assert_eq!(
            store.connection.query_row(
                "SELECT active FROM targets WHERE id = 'TGT-admin'",
                [],
                |row| row.get::<_, i64>(0),
            )?,
            0
        );
        assert_eq!(
            store.connection.query_row(
                "SELECT state FROM target_state WHERE target_id = 'TGT-admin'",
                [],
                |row| row.get::<_, String>(0),
            )?,
            "paused"
        );

        assert!(store.update_target_active("TGT-admin", true)?);
        assert_eq!(
            store.connection.query_row(
                "SELECT state FROM target_state WHERE target_id = 'TGT-admin'",
                [],
                |row| row.get::<_, String>(0),
            )?,
            "unknown"
        );
        assert!(store.delete_target("TGT-admin")?);
        assert!(store.list_targets()?.is_empty());
        assert!(!store.delete_target("TGT-admin")?);

        Ok(())
    }

    #[test]
    fn admin_monitor_create_list_and_delete_are_persistent() -> Result<()> {
        let mut store = migrated_store()?;

        assert!(store.create_monitor(MonitorInsert {
            id: "MON-zeta".to_owned(),
            name: "zeta-worker".to_owned(),
            service: "zeta-worker".to_owned(),
            mode: "ttl".to_owned(),
            expected_every_ms: 60_000,
            grace_ms: 0,
            created_at: "2026-05-28T19:00:00Z".to_owned(),
        })?);
        assert!(store.create_monitor(MonitorInsert {
            id: "MON-alpha".to_owned(),
            name: "alpha-worker".to_owned(),
            service: "".to_owned(),
            mode: "schedule".to_owned(),
            expected_every_ms: 120_000,
            grace_ms: 5_000,
            created_at: "2026-05-28T19:01:00Z".to_owned(),
        })?);

        let monitors = store.list_monitors()?;
        assert_eq!(monitors.len(), 2);
        assert_eq!(monitors[0].id, "MON-alpha");
        assert_eq!(monitors[0].service, "alpha-worker");
        assert_eq!(monitors[1].id, "MON-zeta");
        assert_eq!(
            store.connection.query_row(
                "SELECT state FROM monitor_state WHERE monitor_id = 'MON-alpha'",
                [],
                |row| row.get::<_, String>(0),
            )?,
            "unknown"
        );
        assert_eq!(
            store.connection.query_row(
                "SELECT COUNT(*) FROM monitor_state WHERE monitor_id = 'MON-zeta'",
                [],
                |row| row.get::<_, i64>(0),
            )?,
            1
        );

        assert!(!store.create_monitor(MonitorInsert {
            id: "MON-duplicate".to_owned(),
            name: "alpha-worker".to_owned(),
            service: "alpha-worker".to_owned(),
            mode: "ttl".to_owned(),
            expected_every_ms: 30_000,
            grace_ms: 0,
            created_at: "2026-05-28T19:02:00Z".to_owned(),
        })?);
        assert_eq!(store.list_monitors()?.len(), 2);

        assert!(store.delete_monitor("MON-alpha")?);
        assert_eq!(
            store.connection.query_row(
                "SELECT COUNT(*) FROM monitor_state WHERE monitor_id = 'MON-alpha'",
                [],
                |row| row.get::<_, i64>(0),
            )?,
            0
        );
        assert_eq!(store.list_monitors()?.len(), 1);
        assert!(!store.delete_monitor("MON-alpha")?);

        Ok(())
    }

    #[test]
    fn target_health_transition_updates_state_timeline_and_incident_in_one_commit()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let mut store = migrated_store()?;
        let commit = store
            .commit_target_probe(TargetProbeCommit {
                target_id: "TGT-api".to_owned(),
                state: "down".to_owned(),
                consecutive_failures: 3,
                consecutive_successes: 0,
                check_succeeded: false,
                check: TargetCheckObservation {
                    status_code: Some(503),
                    latency_ms: Some(187),
                    result: "error".to_owned(),
                    tls_expires_at: None,
                    error_detail: Some("expected 200, got 503".to_owned()),
                    region: Some("iad".to_owned()),
                },
                now: "2026-05-28T20:00:00Z".to_owned(),
                transition: Some(TargetTransitionEvent {
                    name: "API".to_owned(),
                    service: "api".to_owned(),
                    url: "https://api.example.com/health".to_owned(),
                    previous_state: "up".to_owned(),
                    event_id: EventId::from_str("EVT-healthdown12")?,
                    incident_id: IncidentId::from_str("INC-healthdown12")?,
                    incident_event_id: EventId::from_str("EVT-incidentdwn1")?,
                }),
            })?
            .transition
            .ok_or("target probe should emit transition")?;

        assert_eq!(commit.event, "health_check.down");
        assert_eq!(
            commit
                .incident_event
                .as_ref()
                .map(|event| event.event.as_str()),
            Some("incident.opened")
        );
        let payload: Value = serde_json::from_str(&commit.payload_json)?;
        assert_eq!(payload["target"]["service"], "api");
        assert_eq!(payload["state"], "down");
        assert_eq!(payload["previous_state"], "up");
        assert_eq!(payload["sequence"], 1);

        let state = store.connection.query_row(
            "SELECT state, consecutive_failures, sequence, last_transition_at
             FROM target_state WHERE target_id = 'TGT-api'",
            [],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, String>(3)?,
                ))
            },
        )?;
        assert_eq!(
            state,
            ("down".to_owned(), 3, 1, "2026-05-28T20:00:00Z".to_owned())
        );
        assert_eq!(
            store.connection.query_row(
                "SELECT event, entity_type, entity_ref, severity, summary
                 FROM service_events WHERE id = 'EVT-healthdown12'",
                [],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, String>(4)?,
                    ))
                },
            )?,
            (
                "health_check.down".to_owned(),
                "target".to_owned(),
                "TGT-api".to_owned(),
                "error".to_owned(),
                "api: API down".to_owned()
            )
        );
        assert_eq!(
            store.connection.query_row(
                "SELECT status_code, latency_ms, result, error_detail, region
                 FROM target_checks WHERE target_id = 'TGT-api'",
                [],
                |row| {
                    Ok((
                        row.get::<_, Option<i64>>(0)?,
                        row.get::<_, Option<i64>>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, Option<String>>(3)?,
                        row.get::<_, Option<String>>(4)?,
                    ))
                },
            )?,
            (
                Some(503),
                Some(187),
                "error".to_owned(),
                Some("expected 200, got 503".to_owned()),
                Some("iad".to_owned())
            )
        );
        assert_eq!(
            store.connection.query_row(
                "SELECT incident_id, signal_type, resolved_at
                 FROM incident_signals WHERE signal_ref = 'TGT-api'",
                [],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, Option<String>>(2)?,
                    ))
                },
            )?,
            (
                "INC-healthdown12".to_owned(),
                "health_transition".to_owned(),
                None
            )
        );

        Ok(())
    }

    #[test]
    fn target_recovery_transition_resolves_the_health_incident_atomically()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let mut store = migrated_store()?;
        store.commit_target_probe(TargetProbeCommit {
            target_id: "TGT-api".to_owned(),
            state: "down".to_owned(),
            consecutive_failures: 3,
            consecutive_successes: 0,
            check_succeeded: false,
            check: TargetCheckObservation {
                status_code: None,
                latency_ms: Some(0),
                result: "connection_error".to_owned(),
                tls_expires_at: None,
                error_detail: Some("connection refused".to_owned()),
                region: None,
            },
            now: "2026-05-28T20:00:00Z".to_owned(),
            transition: Some(TargetTransitionEvent {
                name: "API".to_owned(),
                service: "api".to_owned(),
                url: "https://api.example.com/health".to_owned(),
                previous_state: "up".to_owned(),
                event_id: EventId::from_str("EVT-healthdown12")?,
                incident_id: IncidentId::from_str("INC-healthdown12")?,
                incident_event_id: EventId::from_str("EVT-incidentdwn1")?,
            }),
        })?;

        let recovery = store
            .commit_target_probe(TargetProbeCommit {
                target_id: "TGT-api".to_owned(),
                state: "up".to_owned(),
                consecutive_failures: 0,
                consecutive_successes: 1,
                check_succeeded: true,
                check: TargetCheckObservation {
                    status_code: Some(200),
                    latency_ms: Some(31),
                    result: "ok".to_owned(),
                    tls_expires_at: Some("2026-08-28T00:00:00Z".to_owned()),
                    error_detail: None,
                    region: Some("iad".to_owned()),
                },
                now: "2026-05-28T20:01:00Z".to_owned(),
                transition: Some(TargetTransitionEvent {
                    name: "API".to_owned(),
                    service: "api".to_owned(),
                    url: "https://api.example.com/health".to_owned(),
                    previous_state: "down".to_owned(),
                    event_id: EventId::from_str("EVT-healthup0000")?,
                    incident_id: IncidentId::from_str("INC-unused000001")?,
                    incident_event_id: EventId::from_str("EVT-incidentup01")?,
                }),
            })?
            .transition
            .ok_or("target probe should emit recovery transition")?;

        assert_eq!(recovery.event, "health_check.recovered");
        assert_eq!(
            recovery
                .incident_event
                .as_ref()
                .map(|event| event.event.as_str()),
            Some("incident.resolved")
        );
        assert_eq!(
            store.connection.query_row(
                "SELECT state, resolved_at FROM incidents WHERE id = 'INC-healthdown12'",
                [],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?)),
            )?,
            (
                "resolved".to_owned(),
                Some("2026-05-28T20:01:00Z".to_owned())
            )
        );
        assert_eq!(
            store.connection.query_row(
                "SELECT state, sequence, last_success_at FROM target_state WHERE target_id = 'TGT-api'",
                [],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, Option<String>>(2)?,
                    ))
                },
            )?,
            (
                "up".to_owned(),
                2,
                Some("2026-05-28T20:01:00Z".to_owned())
            )
        );
        assert_eq!(row_count(&store.connection, "target_checks")?, 2);

        Ok(())
    }

    #[test]
    fn target_probe_without_transition_persists_check_and_state_without_timeline()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let mut store = migrated_store()?;

        let commit = store.commit_target_probe(TargetProbeCommit {
            target_id: "TGT-api".to_owned(),
            state: "up".to_owned(),
            consecutive_failures: 0,
            consecutive_successes: 2,
            check_succeeded: true,
            check: TargetCheckObservation {
                status_code: Some(200),
                latency_ms: Some(22),
                result: "success".to_owned(),
                tls_expires_at: None,
                error_detail: None,
                region: Some("iad".to_owned()),
            },
            now: "2026-05-28T20:03:00Z".to_owned(),
            transition: None,
        })?;

        assert!(commit.transition.is_none());
        assert_eq!(row_count(&store.connection, "service_events")?, 0);
        assert_eq!(row_count(&store.connection, "incident_signals")?, 0);
        assert_eq!(
            store.connection.query_row(
                "SELECT state, consecutive_successes, sequence, last_checked_at, last_success_at, last_transition_at
                 FROM target_state WHERE target_id = 'TGT-api'",
                [],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, i64>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, Option<String>>(4)?,
                        row.get::<_, Option<String>>(5)?,
                    ))
                },
            )?,
            (
                "up".to_owned(),
                2,
                0,
                "2026-05-28T20:03:00Z".to_owned(),
                Some("2026-05-28T20:03:00Z".to_owned()),
                None
            )
        );
        assert_eq!(
            store.connection.query_row(
                "SELECT result, status_code, latency_ms, region
                 FROM target_checks WHERE target_id = 'TGT-api'",
                [],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, Option<i64>>(1)?,
                        row.get::<_, Option<i64>>(2)?,
                        row.get::<_, Option<String>>(3)?,
                    ))
                },
            )?,
            (
                "success".to_owned(),
                Some(200),
                Some(22),
                Some("iad".to_owned())
            )
        );

        Ok(())
    }

    #[test]
    fn monitor_health_transition_uses_the_same_incident_boundary()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let mut store = migrated_store()?;
        store.connection.execute(
            "INSERT INTO monitors (id, name, service, mode, expected_every_ms, grace_ms, created_at)
             VALUES ('MON-worker', 'Worker heartbeat', 'worker', 'ttl', 60000, 5000, '2026-05-28T19:00:00Z')",
            [],
        )?;

        let result = store.commit_monitor_check_in(MonitorCheckInCommit {
            monitor_id: "MON-worker".to_owned(),
            state: "degraded".to_owned(),
            last_check_in_at: Some("2026-05-28T20:00:00Z".to_owned()),
            last_check_in_status: Some("alive".to_owned()),
            deadline_at: Some("2026-05-28T20:01:05Z".to_owned()),
            check_in: MonitorCheckInObservation {
                id: "CHK-workeralive0".to_owned(),
                external_id: Some("deploy-42".to_owned()),
                status: "alive".to_owned(),
                observed_at: "2026-05-28T20:00:00Z".to_owned(),
                ttl_ms: Some(60_000),
                summary: Some("worker heartbeat".to_owned()),
                context: Some(r#"{"release":"2026.05.28"}"#.to_owned()),
            },
            now: "2026-05-28T20:02:00Z".to_owned(),
            transition: Some(MonitorTransitionEvent {
                name: "Worker heartbeat".to_owned(),
                service: "worker".to_owned(),
                mode: "ttl".to_owned(),
                expected_every_ms: 60_000,
                grace_ms: 5_000,
                previous_state: "unknown".to_owned(),
                event_id: EventId::from_str("EVT-mondegraded0")?,
                incident_id: IncidentId::from_str("INC-mondegraded0")?,
                incident_event_id: EventId::from_str("EVT-monincident0")?,
            }),
        })?;
        assert_eq!(result.sequence, 1);
        let commit = result
            .transition
            .ok_or("monitor check-in should emit transition")?;

        assert_eq!(commit.event, "health_check.degraded");
        assert_eq!(
            commit
                .incident_event
                .as_ref()
                .map(|event| event.event.as_str()),
            Some("incident.opened")
        );
        let payload: Value = serde_json::from_str(&commit.payload_json)?;
        assert_eq!(payload["monitor"]["mode"], "ttl");
        assert_eq!(payload["last_check_in_status"], "alive");
        assert_eq!(payload["sequence"], 1);
        assert_eq!(
            store.connection.query_row(
                "SELECT state, sequence, last_check_in_status, deadline_at, first_missed_at
                 FROM monitor_state WHERE monitor_id = 'MON-worker'",
                [],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, Option<String>>(2)?,
                        row.get::<_, Option<String>>(3)?,
                        row.get::<_, Option<String>>(4)?,
                    ))
                },
            )?,
            (
                "degraded".to_owned(),
                1,
                Some("alive".to_owned()),
                Some("2026-05-28T20:01:05Z".to_owned()),
                None
            )
        );
        assert_eq!(
            store.connection.query_row(
                "SELECT external_id, status, observed_at, ttl_ms, summary, context
                 FROM monitor_check_ins WHERE id = 'CHK-workeralive0'",
                [],
                |row| {
                    Ok((
                        row.get::<_, Option<String>>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, Option<i64>>(3)?,
                        row.get::<_, Option<String>>(4)?,
                        row.get::<_, Option<String>>(5)?,
                    ))
                },
            )?,
            (
                Some("deploy-42".to_owned()),
                "alive".to_owned(),
                "2026-05-28T20:00:00Z".to_owned(),
                Some(60_000),
                Some("worker heartbeat".to_owned()),
                Some(r#"{"release":"2026.05.28"}"#.to_owned())
            )
        );
        assert_eq!(
            store.connection.query_row(
                "SELECT service, state FROM incidents WHERE id = 'INC-mondegraded0'",
                [],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )?,
            ("worker".to_owned(), "investigating".to_owned())
        );

        Ok(())
    }

    #[test]
    fn monitor_overdue_transition_updates_state_without_check_in_row()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let mut store = migrated_store()?;
        store.connection.execute(
            "INSERT INTO monitors (id, name, service, mode, expected_every_ms, grace_ms, created_at)
             VALUES ('MON-worker', 'Worker heartbeat', 'worker', 'schedule', 60000, 5000, '2026-05-28T19:00:00Z')",
            [],
        )?;
        store.connection.execute(
            "INSERT INTO monitor_state (
                monitor_id, state, last_check_in_status, last_check_in_at, deadline_at, sequence
             ) VALUES (
                'MON-worker', 'up', 'alive', '2026-05-28T19:59:00Z', '2026-05-28T20:00:05Z', 2
             )",
            [],
        )?;

        let result = store.commit_monitor_overdue(MonitorOverdueCommit {
            monitor_id: "MON-worker".to_owned(),
            state: "degraded".to_owned(),
            first_missed_at: Some("2026-05-28T20:01:00Z".to_owned()),
            last_check_in_at: Some("2026-05-28T19:59:00Z".to_owned()),
            last_check_in_status: Some("alive".to_owned()),
            deadline_at: Some("2026-05-28T20:00:05Z".to_owned()),
            now: "2026-05-28T20:01:00Z".to_owned(),
            transition: MonitorTransitionEvent {
                name: "Worker heartbeat".to_owned(),
                service: "worker".to_owned(),
                mode: "schedule".to_owned(),
                expected_every_ms: 60_000,
                grace_ms: 5_000,
                previous_state: "up".to_owned(),
                event_id: EventId::from_str("EVT-mondegraded0")?,
                incident_id: IncidentId::from_str("INC-mondegraded0")?,
                incident_event_id: EventId::from_str("EVT-monincident0")?,
            },
        })?;

        assert_eq!(result.sequence, 3);
        assert_eq!(result.transition.event, "health_check.degraded");
        let payload: Value = serde_json::from_str(&result.transition.payload_json)?;
        assert_eq!(payload["previous_state"], "up");
        assert_eq!(payload["last_check_in_status"], "alive");
        assert_eq!(payload["deadline_at"], "2026-05-28T20:00:05Z");
        assert_eq!(row_count(&store.connection, "monitor_check_ins")?, 0);
        assert_eq!(
            store.connection.query_row(
                "SELECT state, sequence, first_missed_at, last_check_in_status, last_transition_at
                 FROM monitor_state WHERE monitor_id = 'MON-worker'",
                [],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, Option<String>>(2)?,
                        row.get::<_, Option<String>>(3)?,
                        row.get::<_, Option<String>>(4)?,
                    ))
                },
            )?,
            (
                "degraded".to_owned(),
                3,
                Some("2026-05-28T20:01:00Z".to_owned()),
                Some("alive".to_owned()),
                Some("2026-05-28T20:01:00Z".to_owned())
            )
        );
        assert_eq!(
            result
                .transition
                .incident_event
                .as_ref()
                .map(|event| event.event.as_str()),
            Some("incident.opened")
        );

        Ok(())
    }

    #[test]
    fn monitor_overdue_rolls_back_state_when_transition_insert_fails()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let mut store = migrated_store()?;
        store.connection.execute(
            "INSERT INTO monitors (id, name, service, mode, expected_every_ms, grace_ms, created_at)
             VALUES ('MON-worker', 'Worker heartbeat', 'worker', 'schedule', 60000, 5000, '2026-05-28T19:00:00Z')",
            [],
        )?;
        store.connection.execute(
            "INSERT INTO monitor_state (
                monitor_id, state, last_check_in_status, last_check_in_at,
                deadline_at, first_missed_at, sequence
             ) VALUES (
                'MON-worker', 'up', 'alive', '2026-05-28T19:59:00Z',
                '2026-05-28T20:00:05Z', NULL, 2
             )",
            [],
        )?;
        store.connection.execute(
            "INSERT INTO service_events (
                id, service, event, entity_type, entity_ref, severity, summary, payload, created_at
             ) VALUES (
                'EVT-mondegraded0', 'worker', 'health_check.degraded', 'monitor',
                'MON-worker', 'warning', 'duplicate event id', '{}', '2026-05-28T19:59:59Z'
             )",
            [],
        )?;

        let result = store.commit_monitor_overdue(MonitorOverdueCommit {
            monitor_id: "MON-worker".to_owned(),
            state: "degraded".to_owned(),
            first_missed_at: Some("2026-05-28T20:01:00Z".to_owned()),
            last_check_in_at: Some("2026-05-28T19:59:00Z".to_owned()),
            last_check_in_status: Some("alive".to_owned()),
            deadline_at: Some("2026-05-28T20:00:05Z".to_owned()),
            now: "2026-05-28T20:01:00Z".to_owned(),
            transition: MonitorTransitionEvent {
                name: "Worker heartbeat".to_owned(),
                service: "worker".to_owned(),
                mode: "schedule".to_owned(),
                expected_every_ms: 60_000,
                grace_ms: 5_000,
                previous_state: "up".to_owned(),
                event_id: EventId::from_str("EVT-mondegraded0")?,
                incident_id: IncidentId::from_str("INC-mondegraded0")?,
                incident_event_id: EventId::from_str("EVT-monincident0")?,
            },
        });

        assert!(result.is_err());
        assert_eq!(
            store.connection.query_row(
                "SELECT state, sequence, first_missed_at, last_transition_at
                 FROM monitor_state WHERE monitor_id = 'MON-worker'",
                [],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, Option<String>>(2)?,
                        row.get::<_, Option<String>>(3)?,
                    ))
                },
            )?,
            ("up".to_owned(), 2, None, None)
        );
        assert_eq!(
            row_count(&store.connection, "incidents")?,
            0,
            "failed transition insert must not correlate an incident"
        );

        Ok(())
    }

    #[test]
    fn monitor_overdue_candidates_return_deadline_rows_ordered_by_monitor_id() -> Result<()> {
        let mut store = migrated_store()?;
        for (id, deadline_at) in [
            ("MON-b", Some("2026-05-28T20:00:05Z")),
            ("MON-no-deadline", None),
            ("MON-a", Some("2026-05-28T20:00:05Z")),
        ] {
            store.insert_monitor(MonitorInsert {
                id: id.to_owned(),
                name: id.to_owned(),
                service: "worker".to_owned(),
                mode: "schedule".to_owned(),
                expected_every_ms: 60_000,
                grace_ms: 5_000,
                created_at: "2026-05-28T19:00:00Z".to_owned(),
            })?;
            store.connection.execute(
                "INSERT INTO monitor_state (monitor_id, state, last_check_in_status, deadline_at)
                 VALUES (?1, 'up', 'alive', ?2)",
                params![id, deadline_at],
            )?;
        }

        let candidates = store.monitor_overdue_candidates()?;

        assert_eq!(candidates.len(), 2);
        assert_eq!(candidates[0].id, "MON-a");
        assert_eq!(candidates[1].id, "MON-b");
        assert_eq!(candidates[0].state, "up");
        assert_eq!(
            candidates[0].deadline_at.as_deref(),
            Some("2026-05-28T20:00:05Z")
        );
        Ok(())
    }

    #[test]
    fn tls_expiry_scan_candidates_return_active_https_latest_non_null_expiry() -> Result<()> {
        let store = migrated_store()?;
        for (id, url, active) in [
            ("TGT-active", "HTTPS://api.example.test/health", 1),
            ("TGT-http", "http://api.example.test/health", 1),
            ("TGT-inactive", "https://inactive.example.test/health", 0),
        ] {
            store.connection.execute(
                "INSERT INTO targets (
                    id, url, name, service, method, headers, interval_ms, timeout_ms,
                    expected_status, body_contains, degraded_after, down_after, up_after,
                    active, created_at
                 ) VALUES (
                    ?1, ?2, ?1, '', 'GET', NULL, 60000, 10000, '200', NULL,
                    1, 3, 1, ?3, '2026-05-28T20:00:00Z'
                 )",
                params![id, url, active],
            )?;
        }
        store.connection.execute(
            "INSERT INTO target_checks (target_id, checked_at, result, tls_expires_at)
             VALUES
                ('TGT-active', '2026-05-28T20:00:00Z', 'success', '2026-06-01T00:00:00Z'),
                ('TGT-active', '2026-05-29T20:00:00Z', 'success', NULL),
                ('TGT-active', '2026-05-30T20:00:00Z', 'success', '2026-06-05T00:00:00Z'),
                ('TGT-http', '2026-05-30T20:00:00Z', 'success', '2026-06-05T00:00:00Z'),
                ('TGT-inactive', '2026-05-30T20:00:00Z', 'success', '2026-06-05T00:00:00Z')",
            [],
        )?;

        let candidates = store.tls_expiry_scan_candidates()?;

        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].target_id, "TGT-active");
        assert_eq!(candidates[0].service, "TGT-active");
        assert_eq!(candidates[0].tls_expires_at, "2026-06-05T00:00:00Z");
        Ok(())
    }

    #[test]
    fn record_tls_expiring_event_inserts_warning_timeline_payload()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let mut store = migrated_store()?;

        let commit = store.record_tls_expiring_event(TlsExpiryEventInsert {
            event_id: EventId::generate(),
            target_id: "TGT-api".to_owned(),
            name: "api-web".to_owned(),
            service: "api".to_owned(),
            url: "https://api.example.test/healthz".to_owned(),
            tls_expires_at: "2026-06-05T00:00:00Z".to_owned(),
            days_until_expiry: 7,
            now: "2026-05-29T00:00:00Z".to_owned(),
        })?;

        assert_eq!(commit.event, "health_check.tls_expiring");
        let payload: Value = serde_json::from_str(&commit.payload_json)?;
        assert_eq!(payload["target"]["service"], "api");
        assert_eq!(payload["tls_expires_at"], "2026-06-05T00:00:00Z");
        assert_eq!(payload["days_until_expiry"], 7);
        assert_eq!(
            store.connection.query_row(
                "SELECT service, event, entity_type, entity_ref, severity, summary
                 FROM service_events WHERE id = ?1",
                [commit.id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, Option<String>>(3)?,
                        row.get::<_, Option<String>>(4)?,
                        row.get::<_, String>(5)?,
                    ))
                },
            )?,
            (
                "api".to_owned(),
                "health_check.tls_expiring".to_owned(),
                "target".to_owned(),
                Some("TGT-api".to_owned()),
                Some("warning".to_owned()),
                "api: TLS expires in 7 day(s)".to_owned()
            )
        );
        Ok(())
    }

    #[test]
    fn monitor_check_in_without_transition_persists_check_in_and_state_without_timeline()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let mut store = migrated_store()?;
        store.connection.execute(
            "INSERT INTO monitors (id, name, service, mode, expected_every_ms, grace_ms, created_at)
             VALUES ('MON-worker', 'Worker heartbeat', 'worker', 'ttl', 60000, 5000, '2026-05-28T19:00:00Z')",
            [],
        )?;

        let commit = store.commit_monitor_check_in(MonitorCheckInCommit {
            monitor_id: "MON-worker".to_owned(),
            state: "up".to_owned(),
            last_check_in_at: Some("2026-05-28T20:04:00Z".to_owned()),
            last_check_in_status: Some("alive".to_owned()),
            deadline_at: Some("2026-05-28T20:05:05Z".to_owned()),
            check_in: MonitorCheckInObservation {
                id: "CHK-workeralive1".to_owned(),
                external_id: Some("deploy-43".to_owned()),
                status: "alive".to_owned(),
                observed_at: "2026-05-28T20:04:00Z".to_owned(),
                ttl_ms: Some(60_000),
                summary: None,
                context: None,
            },
            now: "2026-05-28T20:04:00Z".to_owned(),
            transition: None,
        })?;

        assert!(commit.transition.is_none());
        assert_eq!(row_count(&store.connection, "service_events")?, 0);
        assert_eq!(row_count(&store.connection, "incident_signals")?, 0);
        assert_eq!(
            store.connection.query_row(
                "SELECT state, sequence, last_check_in_status, deadline_at, last_transition_at
                 FROM monitor_state WHERE monitor_id = 'MON-worker'",
                [],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, Option<String>>(2)?,
                        row.get::<_, Option<String>>(3)?,
                        row.get::<_, Option<String>>(4)?,
                    ))
                },
            )?,
            (
                "up".to_owned(),
                0,
                Some("alive".to_owned()),
                Some("2026-05-28T20:05:05Z".to_owned()),
                None
            )
        );
        assert_eq!(
            store.connection.query_row(
                "SELECT external_id, status, observed_at, ttl_ms
                 FROM monitor_check_ins WHERE id = 'CHK-workeralive1'",
                [],
                |row| {
                    Ok((
                        row.get::<_, Option<String>>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, Option<i64>>(3)?,
                    ))
                },
            )?,
            (
                Some("deploy-43".to_owned()),
                "alive".to_owned(),
                "2026-05-28T20:04:00Z".to_owned(),
                Some(60_000)
            )
        );

        Ok(())
    }

    #[test]
    fn in_progress_monitor_check_in_does_not_update_last_success_at()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let mut store = migrated_store()?;
        store.connection.execute(
            "INSERT INTO monitors (id, name, service, mode, expected_every_ms, grace_ms, created_at)
             VALUES ('MON-worker', 'Worker heartbeat', 'worker', 'ttl', 60000, 5000, '2026-05-28T19:00:00Z')",
            [],
        )?;

        store.commit_monitor_check_in(MonitorCheckInCommit {
            monitor_id: "MON-worker".to_owned(),
            state: "up".to_owned(),
            last_check_in_at: Some("2026-05-28T20:04:00Z".to_owned()),
            last_check_in_status: Some("in_progress".to_owned()),
            deadline_at: Some("2026-05-28T20:05:05Z".to_owned()),
            check_in: MonitorCheckInObservation {
                id: "CHK-workerprogress".to_owned(),
                external_id: None,
                status: "in_progress".to_owned(),
                observed_at: "2026-05-28T20:04:00Z".to_owned(),
                ttl_ms: Some(60_000),
                summary: Some("still running".to_owned()),
                context: None,
            },
            now: "2026-05-28T20:04:00Z".to_owned(),
            transition: None,
        })?;

        assert_eq!(
            store.connection.query_row(
                "SELECT state, last_check_in_status, last_success_at, last_failure_at
                 FROM monitor_state WHERE monitor_id = 'MON-worker'",
                [],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, Option<String>>(1)?,
                        row.get::<_, Option<String>>(2)?,
                        row.get::<_, Option<String>>(3)?,
                    ))
                },
            )?,
            ("up".to_owned(), Some("in_progress".to_owned()), None, None)
        );

        Ok(())
    }

    #[test]
    fn commit_error_ingest_creates_error_group_and_timeline_event() -> Result<()> {
        let mut store = migrated_store()?;
        let ingest = error_ingest(
            "ERR-123456789abc",
            "EVT-123456789abc",
            "group-new",
            "2026-05-28T20:00:00Z",
        );

        let commit = store.commit_error_ingest(ingest)?;

        assert_eq!(commit.id, "ERR-123456789abc");
        assert_eq!(commit.service, "cadence");
        assert_eq!(commit.group_hash, "group-new");
        assert!(commit.is_new_class);
        assert_eq!(
            commit
                .service_event
                .as_ref()
                .map(|event| event.event.as_str()),
            Some("error.new_class")
        );

        assert_eq!(row_count(&store.connection, "errors")?, 1);
        let group_count = store.connection.query_row(
            "SELECT total_count FROM error_groups WHERE group_hash = 'group-new'",
            [],
            |row| row.get::<_, i64>(0),
        )?;
        assert_eq!(group_count, 1);

        let event_payload = store.connection.query_row(
            "SELECT event, entity_type, entity_ref, payload
             FROM service_events
             WHERE entity_ref = 'group-new'",
            [],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                ))
            },
        )?;
        assert_eq!(event_payload.0, "error.new_class");
        assert_eq!(event_payload.1, "error_group");
        assert_eq!(event_payload.2, "group-new");

        let payload_json: Value = serde_json::from_str(&event_payload.3).unwrap_or(Value::Null);
        assert_eq!(payload_json["event"], "error.new_class");
        assert_eq!(payload_json["error"]["id"], "ERR-123456789abc");
        assert_eq!(payload_json["error"]["service"], "cadence");
        assert_eq!(payload_json["error"]["group_hash"], "group-new");

        Ok(())
    }

    #[test]
    fn commit_error_ingest_updates_existing_group_without_new_class_event() -> Result<()> {
        let mut store = migrated_store()?;
        store.commit_error_ingest(error_ingest(
            "ERR-123456789abc",
            "EVT-123456789abc",
            "group-dup",
            "2026-05-28T20:00:00Z",
        ))?;

        let commit = store.commit_error_ingest(error_ingest(
            "ERR-abcdefghijkl",
            "EVT-abcdefghijkl",
            "group-dup",
            "2026-05-28T20:05:00Z",
        ))?;

        assert!(!commit.is_new_class);
        assert!(commit.service_event.is_none());
        let group = store.connection.query_row(
            "SELECT total_count, last_error_id, status
             FROM error_groups
             WHERE group_hash = 'group-dup'",
            [],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            },
        )?;
        assert_eq!(
            group,
            (2, "ERR-abcdefghijkl".to_owned(), "active".to_owned())
        );
        assert_eq!(
            service_event_count(&store.connection, "group-dup", "error.new_class")?,
            1
        );

        Ok(())
    }

    #[test]
    fn commit_error_ingest_records_regression_after_twenty_four_hours() -> Result<()> {
        let mut store = migrated_store()?;
        store.commit_error_ingest(error_ingest(
            "ERR-123456789abc",
            "EVT-123456789abc",
            "group-regression",
            "2026-05-27T20:00:00Z",
        ))?;

        let commit = store.commit_error_ingest(error_ingest(
            "ERR-abcdefghijkl",
            "EVT-abcdefghijkl",
            "group-regression",
            "2026-05-28T20:00:00Z",
        ))?;

        assert!(!commit.is_new_class);
        assert_eq!(
            commit
                .service_event
                .as_ref()
                .map(|event| event.event.as_str()),
            Some("error.regression")
        );
        assert_eq!(
            service_event_count(&store.connection, "group-regression", "error.regression")?,
            1
        );

        Ok(())
    }

    fn migrated_store() -> Result<Store> {
        let mut store = Store::open_in_memory()?;
        store.migrate()?;
        Ok(store)
    }

    fn insert_webhook(
        store: &Store,
        id: &str,
        events: &str,
        active: i64,
        created_at: &str,
    ) -> Result<()> {
        store.connection.execute(
            "INSERT INTO webhooks (id, url, events, secret, active, created_at)
             VALUES (?1, 'https://example.test/hook', ?2, 'secret', ?3, ?4)",
            params![id, events, active, created_at],
        )?;

        Ok(())
    }

    fn error_ingest(
        error_id: &str,
        event_id: &str,
        group_hash: &str,
        created_at: &str,
    ) -> ErrorIngest {
        ErrorIngest {
            ids: ErrorIngestIds {
                error_id: ErrorId::from_str(error_id).unwrap_or_else(|_| ErrorId::generate()),
                event_id: EventId::from_str(event_id).unwrap_or_else(|_| EventId::generate()),
            },
            payload: ErrorIngestPayload {
                service: "cadence".to_owned(),
                error_class: "DBConnection.ConnectionError".to_owned(),
                message: "pool timed out".to_owned(),
                message_template: "pool timed out".to_owned(),
                stack_trace: Some("stack line".to_owned()),
                context_json: Some(r#"{"tenant":"alpha"}"#.to_owned()),
                severity: "warning".to_owned(),
                environment: "production".to_owned(),
                group_hash: group_hash.to_owned(),
                fingerprint_json: Some(r#"["route","handler"]"#.to_owned()),
                region: Some("iad".to_owned()),
                classification: Classification {
                    category: Category::Infrastructure,
                    persistence: Persistence::Transient,
                    component: Component::Database,
                },
                created_at: created_at.to_owned(),
            },
        }
    }

    fn incident_correlation(
        incident_id: &str,
        event_id: &str,
        signal_type: &str,
        signal_ref: &str,
        service: &str,
        now: &str,
    ) -> IncidentCorrelation {
        IncidentCorrelation {
            signal_type: signal_type.to_owned(),
            signal_ref: signal_ref.to_owned(),
            service: service.to_owned(),
            incident_id: IncidentId::from_str(incident_id)
                .unwrap_or_else(|_| IncidentId::generate()),
            event_id: EventId::from_str(event_id).unwrap_or_else(|_| EventId::generate()),
            now: now.to_owned(),
        }
    }

    fn incident_row(
        store: &Store,
        incident_id: &str,
    ) -> Result<(String, String, String, Option<String>)> {
        store
            .connection
            .query_row(
                "SELECT service, state, severity, resolved_at FROM incidents WHERE id = ?1",
                [incident_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .map_err(Into::into)
    }

    fn signal_count(
        store: &Store,
        incident_id: &str,
        signal_type: &str,
        signal_ref: &str,
    ) -> Result<i64> {
        store
            .connection
            .query_row(
                "SELECT count(*) FROM incident_signals
                 WHERE incident_id = ?1 AND signal_type = ?2 AND signal_ref = ?3",
                params![incident_id, signal_type, signal_ref],
                |row| row.get(0),
            )
            .map_err(Into::into)
    }

    fn active_signal_count(store: &Store, incident_id: &str) -> Result<i64> {
        store
            .connection
            .query_row(
                "SELECT count(*) FROM incident_signals
                 WHERE incident_id = ?1 AND resolved_at IS NULL",
                [incident_id],
                |row| row.get(0),
            )
            .map_err(Into::into)
    }

    fn row_count(connection: &Connection, table: &str) -> Result<i64> {
        let count = connection.query_row(&format!("SELECT count(*) FROM {table}"), [], |row| {
            row.get::<_, i64>(0)
        })?;
        Ok(count)
    }

    fn service_event_count(connection: &Connection, group_hash: &str, event: &str) -> Result<i64> {
        let count = connection.query_row(
            "SELECT count(*) FROM service_events WHERE entity_ref = ?1 AND event = ?2",
            params![group_hash, event],
            |row| row.get::<_, i64>(0),
        )?;
        Ok(count)
    }

    fn insert_incident(store: &Store, id: &str, service: &str, opened_at: &str) -> Result<()> {
        store.connection.execute(
            "INSERT INTO incidents (id, service, state, severity, title, opened_at)
             VALUES (?1, ?2, 'investigating', 'medium', ?3, ?4)",
            params![id, service, format!("{service} incident"), opened_at],
        )?;
        Ok(())
    }

    fn insert_incident_signal(
        store: &Store,
        incident_id: &str,
        signal_type: &str,
        signal_ref: &str,
        attached_at: &str,
        resolved_at: Option<&str>,
    ) -> Result<()> {
        store.connection.execute(
            "INSERT INTO incident_signals (
                incident_id, signal_type, signal_ref, attached_at, resolved_at
             ) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                incident_id,
                signal_type,
                signal_ref,
                attached_at,
                resolved_at
            ],
        )?;
        Ok(())
    }

    fn insert_api_key(
        store: &mut Store,
        id: &str,
        raw_key: &str,
        scope: &str,
        revoked_at: Option<&str>,
    ) -> std::result::Result<(), Box<dyn std::error::Error>> {
        store.insert_api_key(ApiKeyInsert {
            id: id.to_owned(),
            name: format!("key {id}"),
            key_prefix: api_keys::key_prefix(raw_key),
            key_hash: bcrypt::hash(raw_key, bcrypt::DEFAULT_COST)?,
            created_at: "2026-05-28T20:00:00Z".to_owned(),
            revoked_at: revoked_at.map(str::to_owned),
            scope: scope.to_owned(),
        })?;
        Ok(())
    }

    fn table_names(connection: &Connection) -> Result<BTreeSet<String>> {
        let mut stmt = connection.prepare(
            "SELECT name
             FROM sqlite_schema
             WHERE type = 'table'
               AND name NOT LIKE 'sqlite_%'
             ORDER BY name",
        )?;
        let names = stmt
            .query_map([], |row| row.get::<_, String>(0))?
            .collect::<rusqlite::Result<BTreeSet<_>>>()?;
        Ok(names)
    }

    fn columns(connection: &Connection, table: &str) -> Result<BTreeMap<String, Column>> {
        let mut stmt = connection.prepare(&format!("PRAGMA table_info({table})"))?;
        let columns = stmt
            .query_map([], |row| {
                Ok(Column {
                    name: row.get(1)?,
                    data_type: row.get(2)?,
                    not_null: row.get::<_, i64>(3)? == 1,
                    default_value: row.get(4)?,
                    primary_key_position: row.get(5)?,
                })
            })?
            .map(|column| column.map(|column| (column.name.clone(), column)))
            .collect::<rusqlite::Result<BTreeMap<_, _>>>()?;
        Ok(columns)
    }

    fn assert_column(columns: &BTreeMap<String, Column>, name: &str, spec: ColumnSpec<'_>) {
        let column = columns.get(name);
        assert!(column.is_some(), "missing column {name}");

        if let Some(column) = column {
            assert_eq!(column.data_type, spec.data_type);
            assert_eq!(column.not_null, spec.not_null);
            assert_eq!(column.default_value.as_deref(), spec.default_value);
            assert_eq!(column.primary_key_position, spec.primary_key_position);
        }
    }

    #[derive(Clone, Copy)]
    struct ColumnSpec<'a> {
        data_type: &'a str,
        not_null: bool,
        default_value: Option<&'a str>,
        primary_key_position: i64,
    }

    impl<'a> ColumnSpec<'a> {
        const fn new(data_type: &'a str) -> Self {
            Self {
                data_type,
                not_null: false,
                default_value: None,
                primary_key_position: 0,
            }
        }

        const fn not_null(mut self) -> Self {
            self.not_null = true;
            self
        }

        const fn default_value(mut self, value: &'a str) -> Self {
            self.default_value = Some(value);
            self
        }

        const fn primary_key_position(mut self, value: i64) -> Self {
            self.primary_key_position = value;
            self
        }
    }

    fn assert_indexes(connection: &Connection, table: &str, expected: &[&str]) -> Result<()> {
        let actual = index_names(connection, table)?;
        for name in expected {
            assert!(actual.contains(*name), "missing index {name}");
        }
        Ok(())
    }

    fn index_names(connection: &Connection, table: &str) -> Result<BTreeSet<String>> {
        let mut stmt = connection.prepare(&format!("PRAGMA index_list({table})"))?;
        let names = stmt
            .query_map([], |row| row.get::<_, String>(1))?
            .collect::<rusqlite::Result<BTreeSet<_>>>()?;
        Ok(names)
    }

    fn trigger_names(connection: &Connection, table: &str) -> Result<BTreeSet<String>> {
        let mut stmt = connection.prepare(
            "SELECT name FROM sqlite_schema
             WHERE type = 'trigger' AND tbl_name = ?1
             ORDER BY name",
        )?;
        let names = stmt
            .query_map([table], |row| row.get::<_, String>(0))?
            .collect::<rusqlite::Result<BTreeSet<_>>>()?;
        Ok(names)
    }

    #[derive(Debug, PartialEq, Eq)]
    struct ForeignKey {
        table: String,
        from: String,
        to: String,
        on_delete: String,
    }

    fn foreign_keys(connection: &Connection, table: &str) -> Result<Vec<ForeignKey>> {
        let mut stmt = connection.prepare(&format!("PRAGMA foreign_key_list({table})"))?;
        let keys = stmt
            .query_map([], |row| {
                Ok(ForeignKey {
                    table: row.get(2)?,
                    from: row.get(3)?,
                    to: row.get(4)?,
                    on_delete: row.get(6)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(keys)
    }
}
