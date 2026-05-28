//! SQLite persistence boundary for the Rust rewrite of Canary.
//!
//! This crate owns the database shape and the single-writer connection. Callers
//! ask it to migrate or persist product operations; they do not assemble SQL
//! from HTTP handlers or worker code.

use std::path::Path;

use rusqlite::Connection;

mod api_keys;
mod ingest;
mod query;
mod schema;

pub use api_keys::{API_KEY_PREFIX_LEN, ApiKeyInsert, VerifiedApiKey};
pub use ingest::{
    ErrorIngest, ErrorIngestCommit, ErrorIngestIds, ErrorIngestPayload, ErrorServiceEvent,
};
pub use query::{IncidentListOptions, QueryError, QueryResult, ServiceQueryOptions};

/// Result type returned by the store boundary.
pub type Result<T> = std::result::Result<T, StoreError>;

/// Persistence-layer failure.
#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    /// SQLite rejected a connection, pragma, migration, or query.
    #[error("sqlite error: {0}")]
    Sqlite(#[from] rusqlite::Error),
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

    /// Insert one API-key row whose raw secret has already been bcrypt-hashed.
    pub fn insert_api_key(&mut self, key: ApiKeyInsert) -> Result<()> {
        api_keys::insert(&self.connection, key)
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

    /// Query recent error counts grouped by error class.
    pub fn errors_by_class(&self, window: &str) -> QueryResult<canary_core::query::ErrorsByClass> {
        query::errors_by_class(&self.connection, window)
    }

    /// Query active incidents with currently active signals.
    pub fn active_incidents(
        &self,
        options: IncidentListOptions,
    ) -> QueryResult<canary_core::query::ActiveIncidents> {
        query::active_incidents(&self.connection, options)
    }

    /// Return one error detail read model.
    pub fn error_detail(
        &self,
        error_id: &str,
    ) -> QueryResult<Option<canary_core::query::ErrorDetail>> {
        query::error_detail(&self.connection, error_id)
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
        ids::{ErrorId, EventId},
        ingest::classification::{Category, Classification, Component, Persistence},
    };
    use rusqlite::params;
    use serde_json::Value;
    use time::OffsetDateTime;

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
